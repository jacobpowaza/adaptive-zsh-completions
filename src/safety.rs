use anyhow::{Context, Result, bail};
use std::{
    ffi::{OsStr, OsString},
    io::Read,
    process::{Command, Stdio},
    time::Duration,
};
use wait_timeout::ChildExt;

const OUTPUT_LIMIT: usize = 2 * 1024 * 1024;

pub fn safe_help_args(path: &[String]) -> Result<Vec<String>> {
    for item in path {
        if item.is_empty()
            || item.len() > 80
            || !item
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
        {
            bail!("unsafe subcommand in discovery path");
        }
    }
    let mut args = path.to_vec();
    args.push("--help".into());
    Ok(args)
}

pub fn run_informational<I, S>(
    program: &str,
    args: I,
    cwd: Option<&std::path::Path>,
    timeout: Duration,
) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_informational_impl(
        program,
        args.into_iter()
            .map(|arg| arg.as_ref().to_owned())
            .collect(),
        cwd,
        timeout,
        false,
    )
}

pub fn run_successful_informational<I, S>(
    program: &str,
    args: I,
    cwd: Option<&std::path::Path>,
    timeout: Duration,
) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_informational_impl(
        program,
        args.into_iter()
            .map(|arg| arg.as_ref().to_owned())
            .collect(),
        cwd,
        timeout,
        true,
    )
}

fn run_informational_impl(
    program: &str,
    args: Vec<OsString>,
    cwd: Option<&std::path::Path>,
    timeout: Duration,
    require_success: bool,
) -> Result<String> {
    if program.contains('\0') || program.is_empty() {
        bail!("invalid executable");
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command
        .env_remove("PAGER")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb");
    let mut child = command
        .spawn()
        .with_context(|| format!("could not run {program}"))?;
    let stdout = child.stdout.take().map(|stream| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stream.take(OUTPUT_LIMIT as u64).read_to_end(&mut bytes);
            bytes
        })
    });
    let stderr = child.stderr.take().map(|stream| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stream.take(OUTPUT_LIMIT as u64).read_to_end(&mut bytes);
            bytes
        })
    });
    let status = child.wait_timeout(timeout)?.ok_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        anyhow::anyhow!("informational command timed out")
    })?;
    let mut bytes = stdout
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    if bytes.is_empty() {
        bytes = stderr
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default();
    }
    if !status.success() && (bytes.is_empty() || require_success) {
        bail!("informational command failed with {status}");
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn sanitize_remote(value: &str) -> Option<String> {
    if value.len() > 1024 || value.contains(['\n', '\r', '\t', '\0', '\u{1b}']) {
        return None;
    }
    if value.chars().any(char::is_control) {
        return None;
    }
    Some(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_terminal_injection() {
        assert!(sanitize_remote("repo\u{1b}[2J").is_none());
        assert!(sanitize_remote("repo\nnext").is_none());
        assert_eq!(
            sanitize_remote("owner/repo.git").as_deref(),
            Some("owner/repo.git")
        );
    }
    #[test]
    fn restricts_discovery_path() {
        assert!(safe_help_args(&["$(touch nope)".into()]).is_err());
    }
    #[test]
    fn strict_informational_calls_reject_error_output() {
        assert!(
            run_successful_informational(
                "sh",
                ["-c", "echo failure >&2; exit 1"],
                None,
                Duration::from_secs(1)
            )
            .is_err()
        );
    }
}
