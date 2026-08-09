use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

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
        "#!/bin/sh\nif [ \"$1\" = serve ]; then\ncat <<'EOF'\nUsage: test-tool serve [OPTIONS]\nOptions:\n  --dangerously-fast  Move fast\nEOF\nelse\ncat <<'EOF'\nUsage: test-tool [COMMAND]\nCommands:\n  serve  Start server\nOptions:\n  -v, --verbose  Verbose\nEOF\nfi\n",
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = cargo_bin_cmd!("adaptive")
        .args(["query", "--buffer", "test-tool serve --dang"])
        .env("PATH", path)
        .env("ADAPTIVE_CACHE_DIR", temp.path().join("cache"))
        .env("ADAPTIVE_DATA_DIR", temp.path().join("data"))
        .env("ADAPTIVE_CONFIG", temp.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["candidates"][0]["value"], "--dangerously-fast");
    assert!(temp.path().join("cache/schemas").is_dir());
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
    fs::write(temp.path().join("profile.txt"), "data").unwrap();
    let directories = query(&temp, "cd pro", temp.path());
    assert_eq!(directories["candidates"][0]["value"], "projects/");

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
fn zsh_frontend_loads_without_overwriting_unrelated_emacs_bindings() {
    let binary = assert_cmd::cargo::cargo_bin!("adaptive");
    let script = format!(
        r#"bindkey -e
before=$(bindkey '^A')
eval "$({} init zsh)"
after=$(bindkey '^A')
[[ "$before" = "$after" ]]
zle -l | grep -E 'adaptive-forward-char|adaptive-menu-tab|adaptive-menu-enter'
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
    assert!(stdout.contains("adaptive-forward-char"));
    assert!(stdout.contains("adaptive-menu-tab"));
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
