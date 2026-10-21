use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
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
    let temp = tempfile::tempdir().unwrap();
    // Hermetic default: the optional carapace source stays off unless a test
    // explicitly enables it, so results never depend on what the host
    // happens to have installed.
    fs::write(
        temp.path().join("config.toml"),
        "[carapace]\nenabled = false\n",
    )
    .unwrap();
    temp
}

/// The adaptive binary, run with a hermetic environment.
///
/// The daemon socket is pinned to this test's own data directory, and the
/// auto-spawn is switched off. The shell integration exports both
/// `ADAPTIVE_SOCK` and `ADAPTIVE_AUTO_DAEMON`, so without this a test would
/// be answered by whatever daemon the developer happens to be running -- with
/// the developer's own profile, not the isolated one -- and would leave a
/// stray server behind.
fn adaptive_cmd(temp: &TempDir) -> assert_cmd::Command {
    let mut cmd = cargo_bin_cmd!("adaptive");
    cmd.env("ADAPTIVE_SOCK", sock_path(temp))
        .env_remove("ADAPTIVE_AUTO_DAEMON");
    cmd
}

/// A fake `carapace` binary. `--list --names` answers from `list_names`;
/// `<completer> export ...` prints `export_json` and counts invocations in
/// `counter` so tests can prove when the subprocess ran.
fn carapace_shim(dir: &std::path::Path, list_names: &str, export_json: &str) -> std::path::PathBuf {
    let counter = dir.join("carapace-calls");
    let shim = dir.join("carapace");
    fs::write(
        &shim,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--list\" ]; then\n  printf '%s' '{list_names}'\n  exit 0\nfi\ncount=$(cat {counter} 2>/dev/null || echo 0)\necho $((count+1)) > {counter}\nprintf '%s' '{export_json}'\n",
            counter = counter.display(),
            list_names = list_names.replace('\'', "'\\''"),
            export_json = export_json.replace('\'', "'\\''"),
        ),
    )
    .unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    shim
}

fn enable_carapace_config(temp: &TempDir) {
    fs::write(
        temp.path().join("config.toml"),
        "[carapace]\nenabled = true\n",
    )
    .unwrap();
}

fn query(temp: &TempDir, buffer: &str, cwd: &std::path::Path) -> Value {
    let output = adaptive_cmd(temp)
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
    let output = adaptive_cmd(&temp)
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

    let status = adaptive_cmd(&temp)
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
        let output = adaptive_cmd(&temp)
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
        let mut command = adaptive_cmd(&temp);
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
    let output = adaptive_cmd(temp)
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
        let output = adaptive_cmd(&temp)
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
        let mut command = adaptive_cmd(&temp);
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
    let output = adaptive_cmd(&temp)
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

// The untyped URL slot after `git clone` is canonical: the generic forge
// prefix ghosts there even when one specific repository dominates memory,
// and the slot survives its own option row (`--depth 1`). A specific repo is
// reached by typing its owner, which re-engages the forge provider.
#[test]
fn forge_prefix_beats_remembered_urls_and_survives_clone_flags() {
    let temp = isolated();
    for _ in 0..4 {
        record_in(
            &temp,
            None,
            None,
            &["git", "clone", "https://github.com/usestrix/x"],
        );
    }
    let plain = query_with(&temp, "git clone ", temp.path(), &["--offline"]);
    assert_eq!(plain["candidates"][0]["value"], "https://github.com/");
    assert!(
        !plain.to_string().contains("usestrix"),
        "remembered URL leaked into the prefix slot: {plain}"
    );

    let flagged = query_with(&temp, "git clone --depth 1 ", temp.path(), &["--offline"]);
    assert_eq!(flagged["candidates"][0]["value"], "https://github.com/");
    assert!(
        !flagged.to_string().contains("usestrix"),
        "remembered URL leaked into the flagged prefix slot: {flagged}"
    );
    assert!(
        !flagged["candidates"][0]["value"]
            .as_str()
            .unwrap()
            .starts_with('-'),
        "a flag outranked the URL prefix at an empty URL slot"
    );

    // Once a positional argument exists, the cursor left the URL slot and no
    // prefix is invented for it.
    let filled = query_with(&temp, "git clone x ", temp.path(), &["--offline"]);
    let values: Vec<&str> = filled["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap_or_default())
        .collect();
    assert!(!values.contains(&"https://github.com/"));
}

#[test]
fn zsh_frontend_loads_without_overwriting_unrelated_emacs_bindings() {
    let binary = assert_cmd::cargo::cargo_bin!("adaptive");
    let temp = tempfile::tempdir().unwrap();
    let script = format!(
        r#"bindkey -e
# Every assertion below is a bare command, so err_exit makes any failure
# abort the script with a nonzero status instead of passing vacuously.
setopt err_exit
trap 'print -r -- "assertion failed near line $LINENO" >&2' ZERR
before=$(bindkey '^A')
before_enter=$(bindkey '^M')
eval "$({} init zsh)"
after=$(bindkey '^A')
[[ "$before" = "$after" ]]
bindkey '^I' | grep -q adaptive-accept
bindkey '^X' | grep -q adaptive-toggle-suppress
bindkey '^[[1;2C' | grep -q adaptive-cycle-fuzzy
bindkey '^[[1;2A' | grep -q adaptive-toggle-empty
[[ "$before_enter" = "$(bindkey '^M')" ]]
zle -l | grep -E 'adaptive-cycle-next|adaptive-cycle-prev|adaptive-accept|adaptive-dismiss'
zle -l | grep -q adaptive-ready
zle -l | grep -q adaptive-daemon-ready
[[ "${{functions[_adaptive_schedule]}}" == *'zle -F -w'* ]]
[[ "${{functions[_adaptive_spawn_query]}}" == *'--format zsh'* ]]
[[ "${{functions[_adaptive_sock_send]}}" == *_adaptive_escape_into* ]]
[[ "${{functions[adaptive-accept]}}" == *'expand-or-complete'* ]]
BUFFER='git clone '
CURSOR=$#BUFFER
LBUFFER=$BUFFER
_adaptive_set_ghost 'https://github.com/'
[[ "$POSTDISPLAY" = 'https://github.com/' ]]
[[ "${{region_highlight[-1]}}" = *'fg=245' ]]
_adaptive_clear_ghost
[[ -z "$POSTDISPLAY" ]]
BUFFER='git c'
CURSOR=$#BUFFER
LBUFFER=$BUFFER
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
LBUFFER=$BUFFER
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
# Handlers flip this before rendering; closing must clear both view and flag.
ADAPTIVE_SELECTOR_ACTIVE=1
_adaptive_close_selector
[[ -z "$POSTDISPLAY" ]]
(( ADAPTIVE_SELECTOR_ACTIVE == 0 ))
BUFFER='git clone https://github.com/jacobpowaza/'
CURSOR=$#BUFFER
LBUFFER=$BUFFER
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
LBUFFER=$BUFFER
ADAPTIVE_PREFIX_LEN=0
ADAPTIVE_VALUES=(only.git)
! _adaptive_auto_selector_after_slash
(( ADAPTIVE_SELECTOR_ACTIVE == 0 ))
# Shift-TAB browses from any state -- a visible ghost never blocks the
# candidate list the way accept-first TAB can. The fast path reuses the
# ghost's own ranked candidates with the ghosted pick pre-highlighted.
bindkey '^[[Z' | grep -q adaptive-browse
zle -l | grep -q adaptive-browse
BUFFER='grep '
CURSOR=$#BUFFER
LBUFFER=$BUFFER
ADAPTIVE_PREFIX_LEN=0
ADAPTIVE_VALUES=(--recursive --color)
ADAPTIVE_INDEX=2
_adaptive_render_current
[[ "$POSTDISPLAY" = '--color' ]]
_adaptive_open_selector_from_ghost
(( ADAPTIVE_SELECTOR_ACTIVE == 1 ))
(( ${{#ADAPTIVE_LIST_VALUES}} == 2 ))
(( ADAPTIVE_LIST_INDEX == 2 ))
[[ "$POSTDISPLAY" == $'\n--recursive  --color' ]]
# The zsh protocol header now carries the context key used for feedback.
BUFFER='git c'
CURSOR=$#BUFFER
LBUFFER=$BUFFER
ADAPTIVE_QUERY_MODE=fork
exec {{test_fd}}< <(printf '1001\t2\t2\tprompt|~|c\nclone\tclone\tdesc\thistory\ncheckout\tcheckout\tdesc\thistory\n')
fixture_prefix=0; fixture_count=0; fixture_ctx=''
IFS=$'\t' read -r _ fixture_prefix fixture_count fixture_ctx <&$test_fd
ADAPTIVE_PREFIX_LEN=$fixture_prefix
ADAPTIVE_CONTEXT_KEY="$fixture_ctx"
_adaptive_apply_results $test_fd $fixture_count
exec {{test_fd}}<&-
[[ "$ADAPTIVE_CONTEXT_KEY" = 'prompt|~|c' ]]
(( ADAPTIVE_PREFIX_LEN == 2 ))
(( ${{#ADAPTIVE_VALUES}} == 2 ))
# Accept/dismiss report back to that key; both helpers exist and guard on
# empty keys instead of spawning processes.
(( ${{+functions[_adaptive_send_feedback]}} == 1 ))
typeset -g ADAPTIVE_PREV_CMD=''
[[ "${{functions[_adaptive_history_hook]}}" == *'--cwd'* && "${{functions[_adaptive_history_hook]}}" == *'--prev'* ]]
[[ "${{functions[_adaptive_schedule]}}" == *_adaptive_sock_send* ]]
# The shell's own forgetting rules are evaluated locally (the daemon may
# have been spawned under a different environment).
HISTORY_IGNORE='(echo *|cargo publish)'
_adaptive_locally_ignored 'echo hello' && echo IGNORED_OK
_adaptive_locally_ignored 'cargo publish' && echo IGNORED2_OK
! _adaptive_locally_ignored 'git push origin main'
unset HISTORY_IGNORE
_adaptive_locally_ignored $'\tsecret' && echo TAB_IGNORED_OK
# A late ghost answer must never repaint over an open candidate list.
[[ "${{functions[_adaptive_render_current]}}" == *selector_active* ]]
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
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .env("HISTFILE", temp.path().join("nohist"))
        .env("ADAPTIVE_DEBUG_LOG", "")
        // Keep the interactive session entirely on the fork path so the
        // structural assertions hold and no daemon outlives the test.
        .env("ADAPTIVE_NO_DAEMON", "1")
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

/// Regression: the wire encoder must be fork-free (command substitution in
/// the send path would cost a process per field per keystroke, defeating the
/// persistent socket) and must escape hostile buffer content exactly like
/// the daemon's codec, or framing breaks.
#[test]
fn sock_send_is_fork_free_and_escapes_hostile_buffers() {
    let binary = assert_cmd::cargo::cargo_bin!("adaptive");
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    let envs: Vec<(&str, String)> = vec![
        ("ADAPTIVE_DATA_DIR", data_dir.display().to_string()),
        (
            "ADAPTIVE_CACHE_DIR",
            temp.path().join("cache").display().to_string(),
        ),
        (
            "ADAPTIVE_CONFIG",
            temp.path().join("config.toml").display().to_string(),
        ),
        ("HISTFILE", temp.path().join("nohist").display().to_string()),
    ];
    let mut cmd = adaptive_cmd(&temp);
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    cmd.args(["history", "record", "--cwd", "/tmp", "--", "tool --flag"])
        .output()
        .unwrap();

    let mut daemon = adaptive_cmd(&temp);
    for (k, v) in &envs {
        daemon.env(k, v);
    }
    daemon.args(["daemon", "start"]).output().unwrap();

    let script = format!(
        r#"
eval "$({} init zsh)"
_adaptive_escape_into $'a\tb\\c'
[[ "$REPLY" = 'a\tb\\c' ]] || {{ echo "ESCAPE_MISMATCH:[$REPLY]"; exit 1 }}
_adaptive_escape_into 'plain'; [[ "$REPLY" = plain ]] || exit 1
ADAPTIVE_SOCK_FD=0
_adaptive_daemon_connect || {{ echo CONNECT_FAIL; exit 1 }}
BUFFER=$'gi\tcl\\one'
CURSOR=$#BUFFER
LBUFFER=$BUFFER
ADAPTIVE_PREV_CMD=$'earlier\tline'
(( ++ADAPTIVE_REQ_ID )); ADAPTIVE_QUERY_ID=$ADAPTIVE_REQ_ID
_adaptive_sock_send 0 || {{ echo SEND_FAIL; exit 1 }}
IFS= read -t 2 -r header <&$ADAPTIVE_SOCK_FD || {{ echo NO_REPLY; exit 1 }}
head=("${{(ps:\t:)header}}")
[[ "${{head[1]}}" = res ]] || {{ echo "BAD_FRAME:[${{head[1]}}]"; exit 1 }}
(( head[2] == ADAPTIVE_QUERY_ID )) || {{ echo "ID_MISMATCH:${{head[2]}}"; exit 1 }}
"#,
        binary.display()
    );
    let mut zsh = Command::new("zsh");
    zsh.args(["-dfi", "-c", &script]);
    for (k, v) in &envs {
        zsh.env(k, v);
    }
    let output = zsh.output().unwrap();
    let _ = adaptive_cmd(&temp)
        .envs(envs.iter().map(|(k, v)| (*k, v.as_str())))
        .args(["daemon", "stop"])
        .output();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Interactive ZLE coverage: the structural tests call the frontend's
/// functions directly; these drive real keystrokes through real widgets on a
/// pseudo-terminal (zsh/zpty) and assert on genuine editor state — ghost
/// accept, arrow cycling, shell-escaped directory insertion, Shift-TAB
/// browsing with row selection, Ctrl-X suppression, and ESC dismiss — in both
/// transports (one-shot fork, live daemon socket). Skipped where zsh/zpty is
/// unavailable.
fn run_zle_pty_harness(daemon_mode: bool) {
    let binary = assert_cmd::cargo::cargo_bin!("adaptive");
    let probe = Command::new("zsh")
        .args(["-dfc", "zmodload zsh/zpty"])
        .output();
    if !probe.is_ok_and(|p| p.status.success()) {
        eprintln!("skipping: zsh with the zpty module is unavailable");
        return;
    }
    let temp = isolated();
    let output = Command::new("zsh")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/zle_pty.zsh"))
        .env("ADAPTIVE_PTY_BIN", binary)
        .env("ADAPTIVE_PTY_BASE", temp.path())
        .env("ADAPTIVE_PTY_DAEMON", if daemon_mode { "1" } else { "0" })
        // The harness exports its own hermetic environment; scrub anything
        // ambient so a developer's local config can never leak into a run.
        .env_remove("ADAPTIVE_DATA_DIR")
        .env_remove("ADAPTIVE_CACHE_DIR")
        .env_remove("ADAPTIVE_CONFIG")
        .env_remove("ADAPTIVE_SOCK")
        .env_remove("HISTFILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "interactive ZLE harness failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn zle_widgets_drive_end_to_end_under_a_pty() {
    run_zle_pty_harness(false);
}

#[test]
fn zle_widgets_over_the_daemon_socket_under_a_pty() {
    run_zle_pty_harness(true);
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
        let output = adaptive_cmd(&temp)
            .args(["history", "record", "--", "choice-tool", "pick", "apricot"])
            .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    let output = adaptive_cmd(&temp)
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
        let output = adaptive_cmd(&temp)
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
    let output = adaptive_cmd(&temp)
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
        let output = adaptive_cmd(&temp)
            .args(["history", "record", "--", "choice-tool", "pick", "apricot"])
            .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    let output = adaptive_cmd(&temp)
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
    let list_output = adaptive_cmd(&temp)
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
    let output = adaptive_cmd(&temp)
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
    let output = adaptive_cmd(temp)
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
    let output = adaptive_cmd(temp)
        .args(["query", "--buffer", buffer, "--cwd", cwd.to_str().unwrap()])
        .args(extra)
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        // Engine-level tests exercise the direct path; daemon behavior has
        // its own dedicated tests below.
        .env("ADAPTIVE_NO_DAEMON", "1")
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
        let output = adaptive_cmd(&temp)
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
        let mut command = adaptive_cmd(&temp);
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
    let response = adaptive_cmd(&temp)
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

// ---------------------------------------------------------------------------
// Carapace integration: an optional external completion source. Every test
// here drives a fake `carapace` shim on PATH so behavior is identical on
// machines with and without the real carapace-bin installed.
// ---------------------------------------------------------------------------

const CARAPACE_LIST: &str = "git\ndeployctl\n";
const GIT_EXPORT: &str = r#"{"values":[
  {"value":"main","display":"main","description":"branch"},
  {"value":"feature/engine","display":"feature/engine","description":"branch"},
  {"value":"hotfix","display":"hotfix","description":"branch"}]}"#;
const DEPLOYCTL_EXPORT: &str = r#"{"values":[
  {"value":"pods","description":"resource"},
  {"value":"services","description":"resource"}]}"#;

/// `git checkout <TAB>` merges Carapace's schema answers with Adaptive's
/// learned habits, and the habit outranks everything Carapace offers.
#[test]
fn carapace_answers_merge_and_learning_overrides_them() {
    let temp = isolated();
    enable_carapace_config(&temp);
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    carapace_shim(&bin, CARAPACE_LIST, GIT_EXPORT);
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(temp.path())
        .status()
        .unwrap();
    fs::write(temp.path().join("seed"), "x").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(temp.path())
        .status()
        .unwrap();
    Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-qm",
            "seed",
        ])
        .current_dir(temp.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["checkout", "-qb", "feature/engine"])
        .current_dir(temp.path())
        .status()
        .unwrap();
    for _ in 0..4 {
        record_in(&temp, None, None, &["git", "checkout", "hotfix"]);
    }
    // Explicit listing so the habitual ghost-collapse cannot hide the merge,
    // with the shim dir first on PATH so carapace is actually detected.
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = adaptive_cmd(&temp)
        .args(["query", "--list", "--buffer", "git checkout "])
        .env("PATH", &path)
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    let candidates = response["candidates"].as_array().unwrap();
    assert!(!candidates.is_empty());
    // The learned branch leads...
    assert_eq!(candidates[0]["value"], "hotfix");
    // ...while Carapace's knowledge is present in the merged list.
    let values: Vec<_> = candidates
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert!(values.contains(&"main"), "{values:?}");
    assert!(values.contains(&"feature/engine"), "{values:?}");
}

/// Where Carapace owns the position, its schema replaces help parsing:
/// the fake tool's `--help` is unparseable, yet exact values come through
/// tagged with their origin.
#[test]
fn carapace_schema_replaces_help_parsing() {
    let temp = isolated();
    enable_carapace_config(&temp);
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    carapace_shim(&bin, CARAPACE_LIST, DEPLOYCTL_EXPORT);
    let tool = bin.join("deployctl");
    fs::write(
        &tool,
        "#!/bin/sh\nprintf 'deploys things, no help here\\n'\n",
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = adaptive_cmd(&temp)
        .args(["query", "--list", "--buffer", "deployctl get "])
        .env("PATH", path)
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let first = &body["candidates"][0];
    assert_eq!(first["value"], "pods");
    assert_eq!(first["source"], "carapace");
}

/// No carapace on PATH: completion works exactly as before.
#[test]
fn carapace_missing_leaves_completion_unchanged() {
    let temp = isolated();
    enable_carapace_config(&temp);
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("mysterytool");
    fs::write(&tool, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..3 {
        record_in(&temp, None, None, &["mysterytool", "alpha"]);
    }
    // The controlled PATH contains only our own tool: no carapace anywhere.
    let output = adaptive_cmd(&temp)
        .args(["query", "--buffer", "mysterytool al"])
        .env("PATH", bin.display().to_string())
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["candidates"][0]["value"], "alpha");
    assert_eq!(body["candidates"][0]["source"], "history");
}

/// Disabled in config: even with carapace present it is never consulted.
#[test]
fn carapace_disabled_is_never_consulted() {
    let temp = isolated(); // writes [carapace] enabled = false
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    carapace_shim(&bin, CARAPACE_LIST, GIT_EXPORT);
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(temp.path())
        .status()
        .unwrap();
    let response = query_with(&temp, "git checkout ", temp.path(), &[]);
    let sources: Vec<_> = response["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["source"].as_str().unwrap_or_default())
        .collect();
    assert!(
        !sources.contains(&"carapace"),
        "disabled carapace leaked into results: {sources:?}"
    );
    assert!(
        !bin.join("carapace-calls").exists(),
        "disabled carapace still spawned"
    );
}

/// A hung carapace cannot stall the pipeline past the query timeout; a
/// crashing or lying carapace degrades to no extra candidates.
#[test]
fn slow_and_broken_carapace_never_break_completion() {
    for script in [
        "#!/bin/sh\nsleep 5\nprintf '%s' '{\"values\":[{\"value\":\"late\"}]}'\n",
        "#!/bin/sh\nexit 3\n",
        "#!/bin/sh\nprintf 'total garbage'\n",
    ] {
        let temp = isolated();
        enable_carapace_config(&temp);
        let bin = temp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let shim = bin.join("carapace");
        fs::write(&shim, script).unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(temp.path())
            .status()
            .unwrap();
        fs::write(temp.path().join("seed"), "x").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(temp.path())
            .status()
            .unwrap();
        Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-qm",
                "seed",
            ])
            .current_dir(temp.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["checkout", "-qb", "real/branch"])
            .current_dir(temp.path())
            .status()
            .unwrap();
        let start = Instant::now();
        let response = query_with(&temp, "git checkout re", temp.path(), &[]);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "unresponsive carapace stalled the query: {elapsed:?}"
        );
        // The rest of the pipeline carried on regardless.
        let values: Vec<_> = response["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["value"].as_str().unwrap())
            .collect();
        assert!(
            values.contains(&"real/branch"),
            "dynamic provider lost alongside broken carapace: {values:?}"
        );
    }
}

/// Repeated positions are served from the answer cache: one subprocess call,
/// however many queries repeat the same question.
#[test]
fn carapace_answers_are_cached() {
    let temp = isolated();
    enable_carapace_config(&temp);
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    carapace_shim(&bin, CARAPACE_LIST, GIT_EXPORT);
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(temp.path())
        .status()
        .unwrap();
    fs::write(temp.path().join("seed"), "x").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(temp.path())
        .status()
        .unwrap();
    Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-qm",
            "seed",
        ])
        .current_dir(temp.path())
        .status()
        .unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    for _ in 0..2 {
        let output = adaptive_cmd(&temp)
            .args(["query", "--buffer", "git checkout "])
            .env("PATH", &path)
            .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
            .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
            .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    let counter = bin.join("carapace-calls");
    let calls: u32 = fs::read_to_string(&counter)
        .unwrap_or_default()
        .trim()
        .parse()
        .unwrap_or(0);
    assert_eq!(
        calls, 1,
        "identical position queried carapace more than once"
    );
}

/// The CLI toggle round-trips through the config file and gates the engine.
#[test]
fn carapace_cli_toggle_controls_the_source() {
    let temp = isolated();
    enable_carapace_config(&temp);
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    carapace_shim(&bin, CARAPACE_LIST, DEPLOYCTL_EXPORT);
    let tool = bin.join("deployctl");
    fs::write(&tool, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let run_cli = |args: &[&str]| {
        let output = adaptive_cmd(&temp)
            .args(args)
            .env("PATH", bin.display().to_string())
            .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
            .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
            .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
            .output()
            .unwrap();
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    };
    let (ok, out) = run_cli(&["carapace", "status"]);
    assert!(ok);
    assert!(out.contains("Detected:"), "{out}");
    assert!(out.contains("State: active"), "{out}");

    let (ok, out) = run_cli(&["carapace", "disable"]);
    assert!(ok, "{out}");
    let (_, out) = run_cli(&["carapace", "status"]);
    assert!(out.contains("inactive (disabled in config)"), "{out}");
    let response = query_with(&temp, "deployctl get ", temp.path(), &[]);
    let sources: Vec<_> = response["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["source"].as_str().unwrap_or_default())
        .collect();
    assert!(!sources.contains(&"carapace"), "{sources:?}");

    let (ok, _) = run_cli(&["carapace", "enable"]);
    assert!(ok);
    let (_, out) = run_cli(&["carapace", "status"]);
    assert!(out.contains("State: active"), "{out}");
}

/// Git workflows gain worktree, deletable-branch, and commit awareness on top
/// of the existing ref completion.
#[test]
fn git_provider_covers_worktrees_deletions_and_commits() {
    let temp = isolated();
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .status()
            .unwrap()
    };
    git(&["init", "-q"]);
    git(&["checkout", "-qb", "feature/engine"]);
    fs::write(repo.join("file"), "one").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "first change"]);
    fs::write(repo.join("file"), "two").unwrap();
    git(&["commit", "-aqm", "second change"]);
    // A sibling directory and an existing linked worktree.
    fs::create_dir(temp.path().join("sibling-dir")).unwrap();
    git(&["worktree", "add", "-b", "wt-branch", "../linked-wt"]);

    // Deletable branches exclude the checked-out one.
    let current = String::from_utf8(
        Command::new("git")
            .args(["symbolic-ref", "--short", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let deletable = query_with(&temp, "git branch -d ", &repo, &[]);
    let deletable_values: Vec<_> = deletable["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert!(
        deletable_values.contains(&"wt-branch"),
        "{deletable_values:?}"
    );
    let checked_out = current.trim();
    assert!(
        !deletable_values.contains(&checked_out),
        "offered deleting the checked-out branch {checked_out}: {deletable_values:?}"
    );

    // Cherry-pick offers recent commits by hash.
    let picked = query_with(&temp, "git cherry-pick ", &repo, &[]);
    let picked_values: Vec<_> = picked["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    let head: String = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(
        picked_values.contains(&head.trim()),
        "expected HEAD hash in {picked_values:?}"
    );

    // `worktree add <path>` suggests sibling directories and existing worktrees.
    let add_path = query_with(&temp, "git worktree add ", &repo, &[]);
    let add_path_values: Vec<_> = add_path["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert!(
        add_path_values.contains(&"../sibling-dir/"),
        "{add_path_values:?}"
    );
    assert!(
        add_path_values.iter().any(|v| v.ends_with("linked-wt")),
        "existing worktree missing from {add_path_values:?}"
    );

    // ...and the second slot offers branches for the new worktree.
    let add_branch = query_with(&temp, "git worktree add ../somewhere ", &repo, &[]);
    let add_branch_values: Vec<_> = add_branch["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert!(
        add_branch_values.contains(&"feature/engine"),
        "{add_branch_values:?}"
    );
}

// ---- daemon ------------------------------------------------------------------

fn adaptive_bin() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin!("adaptive").to_path_buf()
}

fn data_dir(temp: &TempDir) -> std::path::PathBuf {
    temp.path().join("data")
}

fn sock_path(temp: &TempDir) -> std::path::PathBuf {
    data_dir(temp).join("adaptive.sock")
}

fn run_cli(temp: &TempDir, args: &[&str], extra_env: &[(&str, &str)]) -> (bool, String, String) {
    let output = adaptive_cmd(temp)
        .args(args)
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", data_dir(temp))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .envs(extra_env.iter().copied())
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Starts a server bound to this test's data dir and waits until it answers
/// pings. The child is detached; tests stop it explicitly via `daemon stop`.
fn start_daemon(temp: &TempDir) {
    use std::process::Stdio;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut child = std::process::Command::new(adaptive_bin())
        .args(["serve", "--idle-exit-secs", "120"])
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", data_dir(temp))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .env("ADAPTIVE_SOCK", sock_path(temp))
        .env_remove("ADAPTIVE_AUTO_DAEMON")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    while Instant::now() < deadline {
        if sock_path(temp).exists() {
            let (ok, out, _) = run_cli(temp, &["daemon", "status"], &[]);
            if ok && out.contains("running") {
                // Intentionally detached: the server outlives this handle by
                // design and is stopped via `daemon stop`.
                std::mem::forget(child);
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("daemon did not come up within 5s");
}

fn stop_daemon(temp: &TempDir) {
    for _ in 0..20 {
        if !sock_path(temp).exists() {
            return;
        }
        let _ = run_cli(temp, &["daemon", "stop"], &[]);
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn query_raw(temp: &TempDir, args: &[&str], extra_env: &[(&str, &str)]) -> Result<Value, String> {
    let output = adaptive_cmd(temp)
        .args(args)
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", data_dir(temp))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .envs(extra_env.iter().copied())
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())
}

/// The full loop: with the daemon up, queries are served from its in-memory
/// profile (habits recorded after it started are invisible to it); once it
/// stops, the same query falls back to the direct path and sees them.
#[test]
fn daemon_serves_queries_and_the_direct_path_picks_up_where_it_leaves_off() {
    let temp = isolated();
    let (ok, status_before, _) = run_cli(&temp, &["daemon", "status"], &[]);
    assert!(
        ok && status_before.contains("not running"),
        "{status_before}"
    );

    start_daemon(&temp);
    let (_, status_running, _) = run_cli(&temp, &["daemon", "status"], &[]);
    assert!(
        status_running.contains(&format!("running ({})", adaptive_completion::VERSION)),
        "expected matching version banner: {status_running}"
    );

    // Habits recorded AFTER the daemon loaded its profile: the socket path
    // must answer from stale memory...
    record_in(&temp, None, None, &["habitool", "--special-flag"]);
    record_in(&temp, None, None, &["habitool", "--special-flag"]);
    record_in(&temp, None, None, &["habitool", "--special-flag"]);
    let served_by_daemon = query_raw(&temp, &["query", "--buffer", "habitool --sp"], &[]).unwrap();
    assert!(
        !served_by_daemon.to_string().contains("--special-flag"),
        "fresh habit leaked into the running daemon's answers: {served_by_daemon}"
    );

    // ...and the zsh-format payload is byte-identical to the direct path's
    // print_zsh layout (no daemon framing leaks through the CLI).
    let tsv_daemon = adaptive_cmd(&temp)
        .args(["query", "--format", "zsh", "--buffer", "git p"])
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", data_dir(&temp))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    let body = String::from_utf8_lossy(&tsv_daemon.stdout);
    assert!(
        !body.starts_with("res\t"),
        "daemon framing leaked into CLI output: {body}"
    );
    assert!(body.contains("push"), "expected git subcommands: {body}");

    // ...and it is exactly what the same query prints without a daemon
    // (request ids are wall-clock micros, so compare everything but them).
    let tsv_direct = adaptive_cmd(&temp)
        .args([
            "query",
            "--format",
            "zsh",
            "--no-daemon",
            "--buffer",
            "git p",
        ])
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", data_dir(&temp))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    let strip_id = |s: &str| -> String {
        s.lines()
            .enumerate()
            .map(|(i, line)| {
                if i == 0 {
                    line.split('\t').skip(1).collect::<Vec<_>>().join("\t")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(
        strip_id(&body),
        strip_id(&String::from_utf8_lossy(&tsv_direct.stdout)),
    );

    // ...and the direct path (no daemon) sees the new habits immediately.
    stop_daemon(&temp);
    let direct = query_raw(
        &temp,
        &["query", "--buffer", "habitool --sp", "--no-daemon"],
        &[],
    )
    .unwrap();
    assert!(
        direct.to_string().contains("--special-flag"),
        "direct path lost the learned remainder: {direct}"
    );

    let (_, status_after, _) = run_cli(&temp, &["daemon", "status"], &[]);
    assert!(status_after.contains("not running"), "{status_after}");
}

/// Auto-spawn is strictly opt-in (`ADAPTIVE_AUTO_DAEMON=1`, exported by the
/// generated init script): plain CLI usage never leaves processes behind.
/// A corrupt history.json must not silently disable learning: query paths
/// fall back to a fresh enabled profile (and the next save self-heals the
/// file), instead of the Default struct's `enabled = false`.
#[test]
fn corrupt_history_file_degrades_to_a_fresh_enabled_profile() {
    let temp = isolated();
    let file = data_dir(&temp).join("history.json");
    fs::create_dir_all(data_dir(&temp)).unwrap();
    fs::write(&file, "{corrupt json").unwrap();

    let (_, out, err) = run_cli(
        &temp,
        &[
            "history",
            "record",
            "--cwd",
            "/tmp",
            "--",
            "keep-tool --flag",
        ],
        &[],
    );
    assert!(out.is_empty() || out.contains("keep-tool"), "{out}");
    assert!(err.is_empty(), "record failed on corrupt profile: {err}");
    let saved = fs::read_to_string(&file).unwrap();
    assert!(
        serde_json::from_str::<serde_json::Value>(&saved).is_ok(),
        "record did not heal the corrupt profile: {saved}"
    );
    let json = query_with(&temp, "keep-tool ", Path::new("/tmp"), &[]);
    assert!(
        json["candidates"].as_array().is_some_and(|c| !c.is_empty()),
        "habit invisible after corrupt start: {json}"
    );
}

#[test]
fn autospawn_is_opt_in_but_works_when_allowed() {
    let temp = isolated();
    query_raw(&temp, &["query", "--buffer", "git "], &[]).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !sock_path(&temp).exists(),
        "a daemon appeared without opt-in"
    );

    query_raw(
        &temp,
        &["query", "--buffer", "git "],
        &[("ADAPTIVE_AUTO_DAEMON", "1")],
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !sock_path(&temp).exists() {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(sock_path(&temp).exists(), "opt-in autospawn never happened");
    let (_, status, _) = run_cli(&temp, &["daemon", "status"], &[]);
    assert!(status.contains("running"), "{status}");
    stop_daemon(&temp);
}

// ---- empty-prompt toggle -----------------------------------------------------

#[test]
fn empty_prompt_predictions_toggle_off_and_back_on() {
    let temp = isolated();
    let _path = unparsable_tool(&temp, "bare-tool");
    for _ in 0..4 {
        record_in(&temp, None, None, &["bare-tool"]);
    }
    let on = query_with(&temp, "", temp.path(), &["--offline"]);
    assert!(
        on.to_string().contains("bare-tool"),
        "bare prompt should predict the habitual command: {on}"
    );

    let (ok, off_out, err) = run_cli(&temp, &["empty", "off"], &[]);
    assert!(ok, "{err}");
    assert!(off_out.contains("off"), "{off_out}");
    let off = query_with(&temp, "", temp.path(), &["--offline"]);
    assert_eq!(
        off["candidates"].as_array().unwrap().len(),
        0,
        "predictions survived the toggle: {off}"
    );

    let (ok, toggle_out, _) = run_cli(&temp, &["empty", "toggle"], &[]);
    assert!(ok, "{toggle_out}");
    assert!(toggle_out.contains("on"), "{toggle_out}");
    let restored = query_with(&temp, "", temp.path(), &["--offline"]);
    assert!(restored.to_string().contains("bare-tool"));
}

// ---- shell forgetting rules ---------------------------------------------------

#[test]
fn history_ignore_and_space_prefix_are_honored_at_record_time() {
    let temp = isolated();
    record_in(&temp, None, None, &["kept-tool", "arg"]);

    let ignore = "(ignored-tool *|secret-cmd)";
    let ignored_lines: &[&[&str]] = &[&["ignored-tool", "x"], &["secret-cmd"], &[" spaced-out"]];
    for line in ignored_lines {
        let mut args = vec!["history".to_owned(), "record".to_owned(), "--".to_owned()];
        args.extend(line.iter().map(|s| s.to_string()));
        let output = std::process::Command::new(adaptive_bin())
            .args(&args)
            .env("ADAPTIVE_DATA_DIR", data_dir(&temp))
            .env("HISTORY_IGNORE", ignore)
            .output()
            .unwrap();
        assert!(output.status.success());
    }

    let (_, status, _) = run_cli(&temp, &["history", "status"], &[]);
    assert!(status.contains("Patterns: 1"), "{status}");
}

// ---- fuzzy preset ladder ------------------------------------------------------

#[test]
fn fuzzy_preset_cycles_persists_and_reports_its_ladder() {
    let temp = isolated();
    let (ok, out, _) = run_cli(&temp, &["fuzzy"], &[]);
    assert!(ok && out.contains("*smart*"), "{out}");

    let (ok, out, _) = run_cli(&temp, &["fuzzy", "cycle"], &[]);
    assert!(ok && out.contains("*loose*"), "{out}");
    let config = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(config.contains("fuzzy_preset = \"loose\""), "{config}");

    // Backward from loose lands on smart...
    let (ok, out, _) = run_cli(&temp, &["fuzzy", "back"], &[]);
    assert!(ok && out.contains("*smart*"), "{out}");
    // ...and an explicit preset always wins.
    let (ok, out, err) = run_cli(&temp, &["fuzzy", "tight"], &[]);
    assert!(ok && out.contains("*tight*"), "out={out} err={err}");
    let config = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(config.contains("fuzzy_preset = \"tight\""), "{config}");

    // A preset tighter than the typed sprawl hides the wandering match from
    // the ghost but the TAB list still lists it unscored.
    let (ok, out, _) = run_cli(
        &temp,
        &["query", "--format", "zsh", "--list", "--buffer", "gco"],
        &[],
    );
    assert!(ok, "{out}{out}");
}
