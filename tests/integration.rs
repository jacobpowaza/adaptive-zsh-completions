use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use adaptive_completion::discovery::schema_namespace;
use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use tempfile::TempDir;
use tiny_http::{Header, Response, Server};

fn isolated() -> TempDir {
    tempfile::tempdir().unwrap()
}

fn query(temp: &TempDir, buffer: &str, cwd: &std::path::Path) -> Value {
    let output = cargo_bin_cmd!("adaptive")
        .args(["query", "--buffer", buffer, "--cwd", cwd.to_str().unwrap()])
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn generic_help_and_lazy_subcommand_discovery() {
    let temp = isolated();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("test-tool");
    fs::write(
        &tool,
        "#!/bin/sh\nif [ \"$1\" = serve ] && [ \"$2\" = deploy ]; then\ncat <<'EOF'\nUsage: test-tool serve deploy [OPTIONS]\nOptions:\n  --dangerously-fast  Move fast\nEOF\nelif [ \"$1\" = serve ]; then\ncat <<'EOF'\nUsage: test-tool serve [COMMAND]\nCommands:\n  deploy  Deploy server\nEOF\nelse\ncat <<'EOF'\nUsage: test-tool [COMMAND]\nCommands:\n  serve  Start server\nOptions:\n  -v, --verbose  Verbose\nEOF\nfi\n",
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = cargo_bin_cmd!("adaptive")
        .args(["query", "--buffer", "test-tool serve deploy --dang"])
        .env("PATH", path)
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["candidates"][0]["value"], "--dangerously-fast");
    assert!(temp.path().join("cache").join(schema_namespace()).is_dir());
}

#[test]
fn package_json_and_git_branch_providers_are_contextual() {
    let temp = isolated();
    fs::write(
        temp.path().join("package.json"),
        r#"{"scripts":{"dev":"vite","deploy":"node deploy.js"}}"#,
    )
    .unwrap();
    let npm = query(&temp, "npm run de", temp.path());
    let values: Vec<_> = npm["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["value"].as_str().unwrap())
        .collect();
    assert!(values.contains(&"dev") && values.contains(&"deploy"));

    Command::new("git")
        .args(["init", "-q"])
        .current_dir(temp.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["checkout", "-qb", "feature/engine"])
        .current_dir(temp.path())
        .status()
        .unwrap();
    fs::write(temp.path().join("tracked"), "content").unwrap();
    Command::new("git")
        .args(["add", "tracked"])
        .current_dir(temp.path())
        .status()
        .unwrap();
    Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "test",
        ])
        .current_dir(temp.path())
        .status()
        .unwrap();
    let git = query(&temp, "git checkout fea", temp.path());
    assert_eq!(git["candidates"][0]["value"], "feature/engine");
}

#[test]
fn filesystem_and_history_fallback_work_through_protocol() {
    let temp = isolated();
    fs::create_dir(temp.path().join("projects")).unwrap();
    fs::create_dir(temp.path().join("space dir")).unwrap();
    fs::write(temp.path().join("profile.txt"), "data").unwrap();
    let directories = query(&temp, "cd pro", temp.path());
    assert_eq!(directories["candidates"][0]["value"], "projects/");
    let spaced = query(&temp, "cd spa", temp.path());
    assert_eq!(spaced["candidates"][0]["value"], "space\\ dir/");

    let status = cargo_bin_cmd!("adaptive")
        .args(["history", "record", "--", "mysterycmd", "alpha", "beta"])
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .output()
        .unwrap();
    assert!(status.status.success());
    let learned = query(&temp, "mysterycmd al", temp.path());
    assert_eq!(learned["candidates"][0]["value"], "alpha beta");
    assert_eq!(learned["candidates"][0]["source"], "history");
}

#[test]
fn blank_cd_lists_the_current_directory() {
    let temp = isolated();
    fs::create_dir(temp.path().join("Applications")).unwrap();
    fs::create_dir(temp.path().join("Development")).unwrap();
    fs::create_dir(temp.path().join("Downloads")).unwrap();

    for _ in 0..3 {
        let output = cargo_bin_cmd!("adaptive")
            .args(["history", "record", "--", "cd", "Development"])
            .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
            .output()
            .unwrap();
        assert!(output.status.success());
    }

    let response = query(&temp, "cd ", temp.path());
    let values: Vec<_> = response["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|candidate| candidate["value"].as_str().unwrap())
        .collect();
    assert!(values.contains(&"Applications/"));
    assert!(values.contains(&"Development/"));
    assert!(values.contains(&"Downloads/"));
    assert_eq!(values[0], "Development/");
}

#[test]
fn list_query_keeps_sibling_commands_for_an_exact_token() {
    let temp = isolated();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("list-tool");
    fs::write(
        &tool,
        "#!/bin/sh\nprintf 'Usage: list-tool [COMMAND]\\nCommands:\\n  checkout  Switch work\\n  clone  Clone work\\n  commit  Save work\\n  deploy  Deploy work\\n'\n",
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let run = |list: bool| {
        let mut command = cargo_bin_cmd!("adaptive");
        command.args(["query", "--buffer", "list-tool clone"]);
        if list {
            command.arg("--list");
        }
        let output = command
            .env("PATH", &path)
            .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
            .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
            .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
            .output()
            .unwrap();
        assert!(output.status.success());
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let normal = run(false);
    assert_eq!(normal["candidates"].as_array().unwrap().len(), 1);
    let listed = run(true);
    let values: Vec<_> = listed["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|candidate| candidate["value"].as_str().unwrap())
        .collect();
    assert!(values.contains(&"clone"));
    assert!(values.contains(&"checkout"));
    assert!(values.contains(&"commit"));
    assert!(!values.contains(&"deploy"));
}

#[test]
fn git_list_includes_commands_outside_the_short_help_page() {
    let temp = isolated();
    let response = list_query_for_git(&temp, "git clone");
    let values: Vec<_> = response["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|candidate| candidate["value"].as_str().unwrap())
        .collect();
    assert!(values.contains(&"checkout"));
    assert!(values.contains(&"clone"));
    assert!(values.contains(&"commit"));
    assert!(!values.contains(&"cat-file"));
}

fn list_query_for_git(temp: &TempDir, buffer: &str) -> Value {
    let output = cargo_bin_cmd!("adaptive")
        .args(["query", "--list", "--buffer", buffer])
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn executable_change_invalidates_cached_schema() {
    let temp = isolated();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("changing-tool");
    let write_tool = |flag: &str| {
        fs::write(
            &tool,
            format!("#!/bin/sh\nprintf 'Usage: changing-tool [OPTIONS]\\nOptions:\\n  {flag}  Current flag\\n'\n"),
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    };
    write_tool("--first");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let run = |buffer: &str| {
        let output = cargo_bin_cmd!("adaptive")
            .args(["query", "--buffer", buffer])
            .env("PATH", &path)
            .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
            .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
            .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
            .output()
            .unwrap();
        assert!(output.status.success());
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    assert_eq!(
        run("changing-tool --f")["candidates"][0]["value"],
        "--first"
    );
    write_tool("--replacement-longer");
    assert_eq!(
        run("changing-tool --r")["candidates"][0]["value"],
        "--replacement-longer"
    );
}

#[test]
fn github_provider_uses_mock_http_then_works_offline_from_cache() {
    let temp = isolated();
    let server = Server::http("127.0.0.1:0").unwrap();
    let address = format!("http://{}", server.server_addr());
    let (sent, received) = mpsc::channel();
    let handle = thread::spawn(move || {
        let request = server
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        sent.send(request.url().to_owned()).unwrap();
        let body = r#"[{"name":"relay","html_url":"https://github.com/jacobpowaza/relay","clone_url":"https://github.com/jacobpowaza/relay.git","ssh_url":"git@github.com:jacobpowaza/relay.git","description":"Fast relay","fork":false},{"name":"notes","html_url":"https://github.com/jacobpowaza/notes","clone_url":"https://github.com/jacobpowaza/notes.git","ssh_url":"git@github.com:jacobpowaza/notes.git","description":null,"fork":false}]"#;
        request
            .respond(
                Response::from_string(body)
                    .with_header(Header::from_bytes("Content-Type", "application/json").unwrap()),
            )
            .unwrap();
    });
    let run = |offline: bool| {
        let mut command = cargo_bin_cmd!("adaptive");
        command.args([
            "query",
            "--buffer",
            "git clone https://github.com/jacobpowaza/r",
        ]);
        if offline {
            command.arg("--offline");
        }
        command
            .env("ADAPTIVE_GITHUB_API", &address)
            .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
            .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
            .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"));
        let output = command.output().unwrap();
        assert!(output.status.success());
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let online = run(false);
    assert_eq!(
        online["candidates"][0]["value"],
        "https://github.com/jacobpowaza/relay.git"
    );
    assert!(
        received
            .recv()
            .unwrap()
            .starts_with("/users/jacobpowaza/repos")
    );
    handle.join().unwrap();
    let offline = run(true);
    assert_eq!(
        offline["candidates"][0]["value"],
        "https://github.com/jacobpowaza/relay.git"
    );
    assert_eq!(offline["cache_only"], true);
}

#[test]
fn gitlab_forge_provider_uses_the_shared_protocol() {
    let temp = isolated();
    let server = Server::http("127.0.0.1:0").unwrap();
    let address = format!("http://{}", server.server_addr());
    let handle = thread::spawn(move || {
        let request = server
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(request.url().starts_with("/users/example/projects"));
        let body = r#"[{"path":"runner","description":"CI runner"},{"path":"website","description":null}]"#;
        request
            .respond(
                Response::from_string(body)
                    .with_header(Header::from_bytes("Content-Type", "application/json").unwrap()),
            )
            .unwrap();
    });
    let output = cargo_bin_cmd!("adaptive")
        .args([
            "query",
            "--buffer",
            "git clone https://gitlab.com/example/r",
        ])
        .env("ADAPTIVE_GITLAB_API", address)
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        body["candidates"][0]["value"],
        "https://gitlab.com/example/runner.git"
    );
    handle.join().unwrap();
}

#[test]
fn zsh_frontend_loads_without_overwriting_unrelated_emacs_bindings() {
    let binary = assert_cmd::cargo::cargo_bin!("adaptive");
    let script = format!(
        r#"bindkey -e
before=$(bindkey '^A')
before_enter=$(bindkey '^M')
eval "$({} init zsh)"
after=$(bindkey '^A')
[[ "$before" = "$after" ]]
bindkey '^I' | grep -q adaptive-accept
[[ "$before_enter" = "$(bindkey '^M')" ]]
zle -l | grep -E 'adaptive-cycle-next|adaptive-cycle-prev|adaptive-accept|adaptive-dismiss'
zle -l | grep -q adaptive-ready
[[ "${{functions[_adaptive_schedule]}}" == *'zle -F -w'* ]]
[[ "${{functions[adaptive-accept]}}" == *'expand-or-complete'* ]]
BUFFER='git clone '
CURSOR=$#BUFFER
_adaptive_set_ghost 'https://github.com/'
[[ "$POSTDISPLAY" = 'https://github.com/' ]]
[[ "${{region_highlight[-1]}}" = *'fg=245' ]]
_adaptive_clear_ghost
[[ -z "$POSTDISPLAY" ]]
BUFFER='git c'
CURSOR=$#BUFFER
ADAPTIVE_PREFIX_LEN=1
ADAPTIVE_VALUES=(clone checkout commit)
ADAPTIVE_INDEX=1
_adaptive_render_current
[[ "$POSTDISPLAY" = 'lone' ]]
_adaptive_active
ADAPTIVE_INDEX=2
_adaptive_render_current
[[ "$POSTDISPLAY" = 'heckout' ]]
CURSOR=0
! _adaptive_active
BUFFER='grep '
CURSOR=$#BUFFER
ADAPTIVE_PREFIX_LEN=0
ADAPTIVE_VALUES=(--recursive --color)
ADAPTIVE_INDEX=1
! _adaptive_render_current
[[ -z "$POSTDISPLAY" ]]
ADAPTIVE_VALUES=(https://github.com/)
ADAPTIVE_INDEX=1
_adaptive_render_current
[[ "$POSTDISPLAY" = 'https://github.com/' ]]
ADAPTIVE_LIST_VALUES=(Applications Desktop Development)
ADAPTIVE_LIST_DISPLAYS=(Applications/ Desktop/ Development/)
ADAPTIVE_LIST_INDEX=2
_adaptive_render_selector
[[ "$POSTDISPLAY" == $'\nApplications/  Desktop/  Development/' ]]
[[ "${{region_highlight[(r)*standout]}}" == *standout ]]
_adaptive_close_selector
[[ -z "$POSTDISPLAY" ]]
(( ADAPTIVE_SELECTOR_ACTIVE == 0 ))
BUFFER='git clone https://github.com/jacobpowaza/'
CURSOR=$#BUFFER
ADAPTIVE_PREFIX_LEN=0
ADAPTIVE_VALUES=(a.git b.git)
ADAPTIVE_DISPLAYS=(jacobpowaza/a.git jacobpowaza/b.git)
ADAPTIVE_DESCRIPTIONS=('' '')
ADAPTIVE_SOURCES=(dynamic dynamic)
_adaptive_auto_selector_after_slash
(( ADAPTIVE_SELECTOR_ACTIVE == 1 ))
[[ "$POSTDISPLAY" == $'\njacobpowaza/a.git  jacobpowaza/b.git' ]]
_adaptive_reset
BUFFER='git clone https://github.com/jacobpowaza/'
CURSOR=$#BUFFER
ADAPTIVE_PREFIX_LEN=0
ADAPTIVE_VALUES=(only.git)
! _adaptive_auto_selector_after_slash
(( ADAPTIVE_SELECTOR_ACTIVE == 0 ))
"#,
        binary.display()
    );
    let output = Command::new("zsh")
        .args(["-dfi", "-c", &script])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("adaptive-cycle-next"));
    assert!(stdout.contains("adaptive-accept"));
}

#[test]
fn repeated_history_ranks_the_next_argument_first() {
    let temp = isolated();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("choice-tool");
    fs::write(
        &tool,
        "#!/bin/sh\nif [ \"$1\" = pick ]; then\n  printf 'Usage: choice-tool pick [COMMAND]\\nCommands:\\n  apple  First choice\\n  apricot  Learned choice\\n'\nelse\n  printf 'Usage: choice-tool [COMMAND]\\nCommands:\\n  pick  Pick a value\\n'\nfi\n",
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    for _ in 0..3 {
        let output = cargo_bin_cmd!("adaptive")
            .args(["history", "record", "--", "choice-tool", "pick", "apricot"])
            .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    let output = cargo_bin_cmd!("adaptive")
        .args(["query", "--buffer", "choice-tool pick a"])
        .env("PATH", path)
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["candidates"][0]["value"], "apricot");
}

#[test]
fn habitual_next_argument_collapses_to_a_single_ghost_candidate() {
    let temp = isolated();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("choice-tool");
    fs::write(
        &tool,
        "#!/bin/sh\nif [ \"$1\" = pick ]; then\n  printf 'Usage: choice-tool pick [COMMAND]\\nCommands:\\n  apple  First choice\\n  apricot  Learned choice\\n'\nelse\n  printf 'Usage: choice-tool [COMMAND]\\nCommands:\\n  pick  Pick a value\\n'\nfi\n",
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    for _ in 0..4 {
        let output = cargo_bin_cmd!("adaptive")
            .args(["history", "record", "--", "choice-tool", "pick", "apricot"])
            .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    let output = cargo_bin_cmd!("adaptive")
        .args(["query", "--buffer", "choice-tool pick "])
        .env("PATH", &path)
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let candidates = body["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1, "expected only the habitual pick to survive: {candidates:?}");
    assert_eq!(candidates[0]["value"], "apricot");

    // Without an established habit, both choices should still show up (this
    // is the --list / TAB-selector case, and confirms the collapse above is
    // driven by usage, not just "one command has a default").
    let list_output = cargo_bin_cmd!("adaptive")
        .args(["query", "--list", "--buffer", "choice-tool pick "])
        .env("PATH", &path)
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(list_output.status.success());
    let list_body: Value = serde_json::from_slice(&list_output.stdout).unwrap();
    assert_eq!(list_body["candidates"].as_array().unwrap().len(), 2);
}

#[test]
fn zsh_protocol_labels_candidate_sources() {
    let temp = isolated();
    fs::create_dir(temp.path().join("alpha")).unwrap();
    fs::create_dir(temp.path().join("alpine")).unwrap();
    let output = cargo_bin_cmd!("adaptive")
        .args([
            "query",
            "--format",
            "zsh",
            "--buffer",
            "cd al",
            "--cwd",
            temp.path().to_str().unwrap(),
        ])
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let protocol = String::from_utf8(output.stdout).unwrap();
    assert!(
        protocol
            .lines()
            .skip(1)
            .all(|line| line.ends_with("\tfilesystem"))
    );
}

#[test]
fn blank_git_clone_has_one_inline_github_suggestion() {
    let temp = isolated();
    let response = query(&temp, "git clone ", temp.path());
    let candidates = response["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["value"], "https://github.com/");
    assert_eq!(candidates[0]["source"], "dynamic");
}

#[test]
fn warm_local_query_is_fast() {
    let temp = isolated();
    let _ = query(&temp, "git --", temp.path());
    let start = Instant::now();
    let response = query(&temp, "git --v", temp.path());
    assert!(!response["candidates"].as_array().unwrap().is_empty());
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "warm query took {:?}",
        start.elapsed()
    );
}
