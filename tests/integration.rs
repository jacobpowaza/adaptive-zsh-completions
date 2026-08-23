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
# Untyped positions predict proactively: the top-ranked candidate ghosts
# immediately, alternatives stay one arrow-press away.
_adaptive_render_current
[[ "$POSTDISPLAY" = '--recursive' ]]
ADAPTIVE_INDEX=2
_adaptive_render_current
[[ "$POSTDISPLAY" = '--color' ]]
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
# The zsh protocol header now carries the context key used for feedback.
BUFFER='git c'
CURSOR=$#BUFFER
exec {{test_fd}}< <(printf '1\t1\t2\tprompt|~|c\nclone\tclone\tdesc\thistory\ncheckout\tcheckout\tdesc\thistory\n')
_adaptive_load_results $test_fd
exec {{test_fd}}<&-
[[ "$ADAPTIVE_CONTEXT_KEY" = 'prompt|~|c' ]]
(( ADAPTIVE_PREFIX_LEN == 1 ))
(( ${{#ADAPTIVE_VALUES}} == 2 ))
# Accept/dismiss report back to that key; both helpers exist and guard on
# empty keys instead of spawning processes.
(( ${{+functions[_adaptive_send_feedback]}} == 1 ))
typeset -g ADAPTIVE_PREV_CMD=''
[[ "${{functions[_adaptive_history_hook]}}" == *'--cwd'* && "${{functions[_adaptive_history_hook]}}" == *'--prev'* ]]
[[ "${{functions[_adaptive_schedule]}}" == *'--prev'* ]]
# A fresh empty prompt predicts before anything is typed.
BUFFER=''
CURSOR=0
adaptive-line-init
[[ -n "$ADAPTIVE_QUERY_FD" ]]
[[ "$ADAPTIVE_QUERY_BUFFER" = '' ]]
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

/// The prior table is data, not a Git/Homebrew special case: a tool the engine
/// has never heard of gets the same predictive treatment as soon as it has an
/// entry, including at an untyped position.
#[test]
fn global_priors_predict_the_next_token_and_outrank_flags() {
    let temp = isolated();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("widget");
    fs::write(
        &tool,
        "#!/bin/sh\nprintf 'Usage: widget [COMMAND]\\nCommands:\\n  analyze  Analyze it\\n  deploy  Deploy it\\nOptions:\\n  --verbose  Verbose\\n'\n",
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let priors = temp.path().join("priors.toml");
    fs::write(
        &priors,
        "[subcommands]\nwidget = [\"deploy\", \"analyze\"]\n",
    )
    .unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let run = |args: &[&str]| -> Value {
        let output = cargo_bin_cmd!("adaptive")
            .args(args)
            .env("PATH", &path)
            .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
            .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
            .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
            .env("ADAPTIVE_PRIORS", &priors)
            .output()
            .unwrap();
        assert!(output.status.success());
        serde_json::from_slice(&output.stdout).unwrap()
    };

    // Nothing typed after "widget ": the common continuation is predicted
    // outright, rather than waiting for the first character of "deploy".
    let ghost = run(&["query", "--buffer", "widget "]);
    let ghost = ghost["candidates"].as_array().unwrap();
    assert_eq!(ghost.len(), 1, "expected a single prediction: {ghost:?}");
    assert_eq!(ghost[0]["value"], "deploy");

    // TAB at the same position answers with both subcommands first and the
    // flag after -- never the flag alone.
    let list = run(&["query", "--list", "--buffer", "widget "]);
    let values: Vec<&str> = list["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert_eq!(values, ["deploy", "analyze", "--verbose"]);
}

/// Plenty of CLIs answer `--help` with nothing parseable. A known tool should
/// still complete: the prior table is a source of candidates, not only an
/// ordering hint.
#[test]
fn priors_supply_candidates_when_discovery_finds_nothing() {
    let temp = isolated();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("mystery");
    fs::write(&tool, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let priors = temp.path().join("priors.toml");
    fs::write(&priors, "[subcommands]\nmystery = [\"ship\", \"revert\"]\n").unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = cargo_bin_cmd!("adaptive")
        .args(["query", "--list", "--buffer", "mystery "])
        .env("PATH", &path)
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .env("ADAPTIVE_PRIORS", &priors)
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let values: Vec<&str> = body["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert_eq!(values, ["ship", "revert"]);
    assert_eq!(body["candidates"][0]["source"], "prior");
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
    assert_eq!(
        candidates.len(),
        1,
        "expected only the habitual pick to survive: {candidates:?}"
    );
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

/// Records a command line as if executed from `cwd` with `prev` having run
/// just before it.
fn record_in(temp: &TempDir, cwd: Option<&str>, prev: Option<&str>, line: &[&str]) {
    let mut args = vec!["history".to_owned(), "record".to_owned()];
    if let Some(cwd) = cwd {
        args.push("--cwd".to_owned());
        args.push(cwd.to_owned());
    }
    if let Some(prev) = prev {
        args.push("--prev".to_owned());
        args.push(prev.to_owned());
    }
    args.push("--".to_owned());
    args.extend(line.iter().map(|s| s.to_string()));
    let output = cargo_bin_cmd!("adaptive")
        .args(&args)
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A tool whose help cannot be parsed: predictions must come from learned
/// usage alone, proving memory works without any documentation.
fn unparsable_tool(temp: &TempDir, name: &str) -> String {
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let tool = bin.join(name);
    fs::write(&tool, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap())
}

fn query_with(temp: &TempDir, buffer: &str, cwd: &std::path::Path, extra: &[&str]) -> Value {
    let output = cargo_bin_cmd!("adaptive")
        .args(["query", "--buffer", buffer, "--cwd", cwd.to_str().unwrap()])
        .args(extra)
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

// 1 + 2. Trailing space is a completion event: after repeating one flag a
// couple of times, typing the command and pressing Space predicts it
// immediately, and the learned memory survives into later processes.
#[test]
fn trailing_space_predicts_learned_flag_without_typing() {
    let temp = isolated();
    let _path = unparsable_tool(&temp, "deploy-tool");
    for _ in 0..3 {
        record_in(&temp, None, None, &["deploy-tool", "--dangerously-fast"]);
    }
    // Separate process (persistence across sessions): the prediction comes
    // back collapsed to exactly one confident candidate.
    let response = query_with(&temp, "deploy-tool ", temp.path(), &["--offline"]);
    let candidates = response["candidates"].as_array().unwrap();
    assert_eq!(
        candidates.len(),
        1,
        "want a single proactive pick: {candidates:?}"
    );
    assert_eq!(candidates[0]["value"], "--dangerously-fast");
}

// 2. The same flag habit ranks first in the TAB list too, above whatever
// generic help would offer.
#[test]
fn remembered_command_flag_combinations_rank_first() {
    let temp = isolated();
    let _path = unparsable_tool(&temp, "deploy-tool");
    record_in(&temp, None, None, &["deploy-tool", "--dangerously-fast"]);
    record_in(&temp, None, None, &["deploy-tool", "--dry-run"]);
    record_in(&temp, None, None, &["deploy-tool", "--dangerously-fast"]);
    record_in(&temp, None, None, &["deploy-tool", "--dangerously-fast"]);
    let response = query_with(&temp, "deploy-tool ", temp.path(), &["--list"]);
    let values: Vec<_> = response["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert_eq!(values[0], "--dangerously-fast");
    assert!(values.contains(&"--dry-run"));
}

// 3. Empty prompt: before anything is typed, the most frequent command for
// this directory is predicted.
#[test]
fn empty_prompt_predicts_the_directory_habit() {
    let temp = isolated();
    fs::create_dir(temp.path().join("shop")).unwrap();
    let project = temp.path().join("shop");
    for _ in 0..4 {
        record_in(
            &temp,
            Some(project.to_str().unwrap()),
            None,
            &["npm", "run", "dev"],
        );
    }
    let response = query_with(&temp, "", &project, &[]);
    let candidates = response["candidates"].as_array().unwrap();
    assert!(!candidates.is_empty());
    assert_eq!(candidates[0]["value"], "npm");
}

// 5. Command sequences: what usually follows the previous line shapes the
// next prediction.
#[test]
fn previous_command_sequence_shapes_prediction() {
    let temp = isolated();
    fs::create_dir(temp.path().join("api")).unwrap();
    let project = temp.path().join("api");
    for _ in 0..3 {
        record_in(&temp, None, None, &["git", "pull", "origin", "main"]);
        record_in(
            &temp,
            Some(project.to_str().unwrap()),
            Some("git pull origin main"),
            &["cargo", "test"],
        );
        // Noise: an unrelated command that must not outrank the sequence.
        record_in(&temp, None, Some("echo hi"), &["ls"]);
    }
    let response = query_with(&temp, "", temp.path(), &["--prev", "git pull origin main"]);
    let candidates = response["candidates"].as_array().unwrap();
    assert!(!candidates.is_empty());
    assert_eq!(candidates[0]["value"], "cargo");
}

// 8. Accept/reject feedback moves rankings both directions.
#[test]
fn accept_and_reject_feedback_reorder_rankings() {
    let temp = isolated();
    let _path = unparsable_tool(&temp, "choice-tool");
    // apricot starts ahead on raw frequency.
    record_in(&temp, None, None, &["choice-tool", "pick", "apricot"]);
    record_in(&temp, None, None, &["choice-tool", "pick", "apple"]);
    record_in(&temp, None, None, &["choice-tool", "pick", "apricot"]);
    let feedback = |kind: &str, value: &str| {
        let output = cargo_bin_cmd!("adaptive")
            .args([
                "history",
                "feedback",
                kind,
                "--context",
                "choice-tool pick",
                "--value",
                value,
            ])
            .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
            .output()
            .unwrap();
        assert!(output.status.success());
    };
    feedback("reject", "apricot");
    feedback("reject", "apricot");
    feedback("accept", "apple");
    feedback("accept", "apple");
    let response = query_with(&temp, "choice-tool pick ", temp.path(), &["--list"]);
    let values: Vec<_> = response["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert_eq!(
        values[0], "apple",
        "acceptance overtook raw frequency: {values:?}"
    );
    assert!(
        values.contains(&"apricot"),
        "rejection hides nothing from TAB"
    );

    let ghost = query_with(&temp, "choice-tool pick ", temp.path(), &[]);
    let ghost_values: Vec<_> = ghost["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert_eq!(ghost_values[0], "apple");
}

// 4. Multi-token inline completion: a typed prefix completes to the whole
// remembered line, then keeps predicting after each accepted token.
#[test]
fn chain_prediction_completes_entire_learned_line() {
    let temp = isolated();
    let _path = unparsable_tool(&temp, "notebook");
    for _ in 0..3 {
        record_in(&temp, None, None, &["notebook", "sync", "--all-branches"]);
    }
    // Typing only "not" ghosts the entire remainder of the habitual line.
    let head = query_with(&temp, "not", temp.path(), &[]);
    assert_eq!(
        head["candidates"][0]["value"],
        "notebook sync --all-branches"
    );
    // After the subcommand token, the chain continues to its arguments.
    let tail = query_with(&temp, "notebook sync ", temp.path(), &[]);
    assert_eq!(tail["candidates"][0]["value"], "--all-branches");
}

// 6. Project context plus usage: inside one repo claude leads, elsewhere the
// directory's own habit wins.
#[test]
fn per_directory_commands_diverge_by_context() {
    let temp = isolated();
    fs::create_dir(temp.path().join("web")).unwrap();
    fs::create_dir(temp.path().join("ops")).unwrap();
    let web = temp.path().join("web");
    let ops = temp.path().join("ops");
    for _ in 0..4 {
        record_in(&temp, Some(web.to_str().unwrap()), None, &["claude"]);
    }
    for _ in 0..4 {
        record_in(&temp, Some(ops.to_str().unwrap()), None, &["kubectl"]);
    }
    let in_web = query_with(&temp, "", &web, &[])["candidates"][0]["value"]
        .as_str()
        .unwrap()
        .to_owned();
    let in_ops = query_with(&temp, "", &ops, &[])["candidates"][0]["value"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(in_web, "claude");
    assert_eq!(in_ops, "kubectl");
}

// 7. History import bootstraps the model so a fresh install already knows
// the user's habits.
#[test]
fn history_import_builds_predictions_from_shell_history() {
    let temp = isolated();
    let histfile = temp.path().join("zsh_history");
    fs::write(
        &histfile,
        ": 1720000001:0;claude --dangerously-skip-permissions\n\
         : 1720000002:0;claude --dangerously-skip-permissions\n\
         : 1720000003:0;git status\n",
    )
    .unwrap();
    let data_dir = temp.path().join("data");
    let import = |extra: &[&str]| {
        let mut command = cargo_bin_cmd!("adaptive");
        command
            .args(["history", "import"])
            .arg(histfile.to_str().unwrap())
            .env("ADAPTIVE_DATA_DIR", &data_dir);
        for arg in extra {
            command.arg(arg);
        }
        let out = command.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let first = import(&[]);
    assert!(first.starts_with("Imported 3 lines"), "{first}");
    // Idempotent: a second shell start must not double-count.
    let second = import(&[]);
    assert!(second.starts_with("Already imported"), "{second}");

    let path = unparsable_tool(&temp, "claude");
    let response = cargo_bin_cmd!("adaptive")
        .args([
            "query",
            "--buffer",
            "claude ",
            "--cwd",
            temp.path().to_str().unwrap(),
        ])
        .env("PATH", path)
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", &data_dir)
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    let body: Value = serde_json::from_slice(&response.stdout).unwrap();
    assert_eq!(
        body["candidates"][0]["value"],
        "--dangerously-skip-permissions"
    );
}

// 9. Accepting a token keeps the chain going: the follow-up query at the new
// position carries a fresh prediction.
#[test]
fn accepting_a_token_chains_into_the_next_prediction() {
    let temp = isolated();
    let _path = unparsable_tool(&temp, "stack");
    record_in(&temp, None, None, &["stack", "deploy", "web"]);
    record_in(&temp, None, None, &["stack", "deploy", "web"]);
    record_in(&temp, None, None, &["stack", "rollback", "db"]);
    // After typing+space following "deploy", the argument slot predicts.
    let step = query_with(&temp, "stack deploy ", temp.path(), &["--offline"]);
    let candidates = step["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1, "{candidates:?}");
    assert_eq!(candidates[0]["value"], "web");
    // And the engine echoes the position key the frontend needs to send
    // acceptance feedback back to this exact slot.
    assert_eq!(step["context_key"], "stack deploy");
}

// 10. Prediction stays fast even once the profile has plenty of entries.
#[test]
fn predictive_queries_stay_fast_with_a_populated_profile() {
    let temp = isolated();
    let _path = unparsable_tool(&temp, "predictool");
    for i in 0..200 {
        record_in(
            &temp,
            None,
            None,
            &["predictool", &format!("sub{i}"), "value{i}"],
        );
    }
    for _ in 0..3 {
        record_in(&temp, None, None, &["predictool", "favorite-sub"]);
    }
    let _ = query_with(&temp, "predictool ", temp.path(), &["--offline"]);
    let start = Instant::now();
    let response = query_with(&temp, "predictool ", temp.path(), &["--offline"]);
    assert_eq!(response["candidates"][0]["value"], "favorite-sub");
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "prediction took {:?}",
        start.elapsed()
    );
}

// Documentation is consulted even where a dynamic provider owns the
// position: typing a brand-new flag must read the command's own settings
// list instead of coming back empty just because branches/remotes/scripts
// live here too.
#[test]
fn new_flag_prefixes_read_the_command_settings_list() {
    let temp = isolated();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(temp.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["remote", "add", "origin", "https://github.com/x/y.git"])
        .current_dir(temp.path())
        .status()
        .unwrap();

    // A flag never used before, on a position a provider covers.
    let response = query_with(&temp, "git push --for", temp.path(), &[]);
    let values: Vec<_> = response["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert!(
        values.contains(&"--force-with-lease") || values.contains(&"--force"),
        "documented flags suggested for a brand-new prefix: {values:?}"
    );

    // And the explicit TAB listing at a provider-covered slot also includes
    // the documented options of that subcommand, not only its dynamic
    // values.
    let listed = query_with(&temp, "git push ", temp.path(), &["--list"]);
    let listed_values: Vec<&str> = listed["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert!(
        listed_values.iter().any(|v| v.starts_with("--")),
        "TAB list includes documented settings alongside dynamic ones: {listed_values:?}"
    );
}
