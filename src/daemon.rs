//! Long-lived completion server ("daemon").
//!
//! One process serves every terminal: it loads the usage profile once into
//! memory behind an `RwLock` and answers queries over a Unix-domain socket,
//! so a keystroke costs a socket round trip (well under a millisecond warm)
//! instead of a fork/exec plus a profile re-parse. The wire protocol is the
//! same sanitized tab-separated format the zsh frontend already parses --
//! one escaped TSV line per request, zero or more per response.
//!
//! Requests (fields backslash-escaped):
//!
//! ```text
//! ping\t<id>
//! query\t<id>\t<0|1 list>\t<buffer>\t<cursor>\t<cwd>\t<prev>
//! record\t<id>\t<cwd>\t<prev>\t<line>
//! feedback\t<id>\t<accept|reject>\t<context>\t<value>
//! settings\t<id>
//! set_fuzzy\t<id>\t<tight|smart|loose>
//! set_empty\t<id>\t<on|off|toggle>
//! reload\t<id>
//! shutdown\t<id>
//! ```
//!
//! Responses: `ok/<err>` first field carries the echoed id; `query` answers
//! either with compact JSON (`jres\t<id>\t<json>`) or, for the zsh
//! frontend's framing, `res\t<id>\t<request_id>\t<prefix_len>\t<count>\t<context_key>`
//! followed by `<count>` rows of `<value>\t<display>\t<desc>\t<source>`.
//!
//! The server saves mutated state straight through to disk (atomic temp +
//! rename, as everywhere else), clamps its data directory and socket to
//! owner-only access, and exits quietly when another healthy instance owns
//! the socket. An idle timer stops a forgotten daemon so nothing is left
//! running after the last shell closes.

use crate::{
    VERSION,
    cache::Cache,
    config::{Config, FuzzyPreset},
    engine::Engine,
    history::{self, FeedbackKind, HistoryDb},
    model::QueryResponse,
};
use anyhow::{Context, Result};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

/// Default time a server stays alive without serving a request.
pub const DEFAULT_IDLE_EXIT: Duration = Duration::from_secs(15 * 60);

// ---- wire codec -------------------------------------------------------------

/// Escapes one TSV field: backslash, tab, newline and carriage return become
/// their backslash forms; other control characters are dropped (mirroring
/// the sanitizer applied to every outgoing value).
pub fn escape_field(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Inverse of [`escape_field`] (unknown escapes resolve to the literal char).
pub fn unescape_field(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn split_line(line: &str) -> Vec<String> {
    line.split('\t').map(unescape_field).collect()
}

// ---- server -----------------------------------------------------------------

struct ServerState {
    /// Shared profile snapshot. Readers clone the `Arc` (cheap) and release
    /// the lock immediately, so a query never holds the lock across the
    /// provider subprocesses it may spawn; writers build a fresh snapshot
    /// and swap it in.
    history: RwLock<std::sync::Arc<HistoryDb>>,
    config: Mutex<Config>,
    cache: Cache,
    last_activity: Mutex<Instant>,
    shutdown: std::sync::atomic::AtomicBool,
}

impl ServerState {
    fn touch(&self) {
        *self.locked_activity() = Instant::now();
    }
    fn locked_activity(&self) -> std::sync::MutexGuard<'_, Instant> {
        self.last_activity.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Runs the server until `idle_exit` passes with no requests (or a
/// `shutdown` op arrives). Returns once the socket file has been removed.
pub fn serve(idle_exit: Duration) -> Result<()> {
    // Load configuration before touching the socket so a broken config
    // cannot leave a bound-but-unlistened socket file behind.
    let config = Config::load()?;
    let sock_path = history::socket_path();
    if let Some(parent) = sock_path.parent() {
        history::ensure_private_dir(&parent.to_path_buf());
    }
    if sock_path.exists() {
        // Another healthy daemon owns the socket: leave quietly instead of
        // stealing it. A stale file (crashed owner) is removed and rebound.
        if UnixStream::connect(&sock_path).is_ok() {
            return Ok(());
        }
        let _ = std::fs::remove_file(&sock_path);
    }
    let listener = UnixListener::bind(&sock_path)
        .with_context(|| format!("could not bind {}", sock_path.display()))?;
    let _ = std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600));
    // Blocking accept: a keystroke must be answered immediately, not on the
    // next polling tick. A watchdog thread below handles idleness.
    listener.set_nonblocking(false)?;

    let state = Arc::new(ServerState {
        history: RwLock::new(std::sync::Arc::new(HistoryDb::load_or_fresh())),
        config: Mutex::new(config),
        cache: Cache::new(),
        last_activity: Mutex::new(Instant::now()),
        shutdown: std::sync::atomic::AtomicBool::new(false),
    });

    // Watchdog: flips the stop flag and wakes the blocked acceptor (by
    // dialing our own socket) when idle time expires.
    {
        let state = Arc::clone(&state);
        let wake_path = sock_path.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(500));
                if state.shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                if state.locked_activity().elapsed() >= idle_exit {
                    state
                        .shutdown
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    let _ = UnixStream::connect(&wake_path);
                    return;
                }
            }
        });
    }

    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if state.shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                    drop(stream);
                    break;
                }
                state.touch();
                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    let _ = handle_connection(stream, &state);
                    state.touch();
                });
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => {
                // Transient accept errors are logged-by-omission; the loop
                // continues unless a stop was requested.
            }
        }
        if state.shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
    }
    let _ = std::fs::remove_file(&sock_path);
    // No save-on-exit: every mutation already persisted straight through,
    // while a blind final save could clobber habits written directly to disk
    // by CLI invocations after this process loaded its snapshot.
    Ok(())
}

fn handle_connection(stream: UnixStream, state: &ServerState) -> Result<()> {
    let writer = stream.try_clone()?;
    let mut writer = writer;
    // A client that stops draining its socket must not pin this thread
    // forever in a blocking write; five seconds covers any real consumer.
    writer.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.is_empty() {
            continue;
        }
        state.touch();
        if split_line(trimmed).first().map(String::as_str) == Some("shutdown") {
            state
                .shutdown
                .store(true, std::sync::atomic::Ordering::SeqCst);
            writeln!(writer, "ok\t0\tbye")?;
            writer.flush()?;
            // The acceptor sits blocked in accept(); dial our own socket so
            // it observes the stop flag immediately instead of waiting for
            // some future client.
            let _ = UnixStream::connect(history::socket_path());
            return Err(anyhow::anyhow!("shutdown requested"));
        }
        for response in dispatch(trimmed, state) {
            writeln!(writer, "{response}")?;
        }
        writer.flush()?;
        state.touch();
    }
}

/// Answers one request line with zero or more response lines. Errors are
/// reported in-band (`err\t<id>\t<message>`) so clients can fall back
/// without ever seeing a broken protocol.
fn dispatch(line: &str, state: &ServerState) -> Vec<String> {
    let fields = split_line(line);
    let id = fields.get(1).cloned().unwrap_or_else(|| "0".into());
    let ok = |extra: &[String]| {
        let mut out = vec!["ok".to_owned(), id.clone()];
        out.extend_from_slice(extra);
        vec![out.join("\t")]
    };
    match fields.first().map(String::as_str) {
        Some("ping") => ok(&["pong".to_owned(), VERSION.to_owned()]),
        Some("query") => match handle_query(&fields, state) {
            Ok(lines) => lines,
            Err(e) => vec![format!("err\t{id}\t{}", escape_field(&format!("{e:#}")))],
        },
        Some("record") => {
            let cwd = fields.get(2).map(String::as_str);
            let prev = fields.get(3).map(String::as_str);
            let line_cmd = fields.get(4).cloned().unwrap_or_default();
            mutate_history(&id, state, |h| {
                h.record(&line_cmd, cwd.filter(|c| !c.is_empty()), prev)
            })
        }
        Some("feedback") => {
            let kind = match fields.get(2).map(String::as_str) {
                Some("accept") => FeedbackKind::Accept,
                _ => FeedbackKind::Reject,
            };
            let context = fields.get(3).cloned().unwrap_or_default();
            let value = fields.get(4).cloned().unwrap_or_default();
            mutate_history(&id, state, |h| h.feedback(kind, &context, &value))
        }
        Some("settings") => {
            let config = state
                .config
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            ok(&[
                config.completion.fuzzy_preset.clone(),
                bool_word(config.ui.empty_prompt_predictions).to_owned(),
                bool_word(config.completion.fuzzy).to_owned(),
                bool_word(config.completion.predictive).to_owned(),
            ])
        }
        Some("set_fuzzy") => {
            let Some(preset) = fields.get(2).and_then(|v| FuzzyPreset::parse(v)) else {
                return vec![format!("err\t{id}\tunknown preset")];
            };
            apply_config_change(state, |config| {
                config.completion.fuzzy = true;
                config.completion.fuzzy_preset = preset.as_str().to_owned();
            });
            ok(&[preset.as_str().to_owned()])
        }
        Some("set_empty") => {
            let requested = fields.get(2).cloned().unwrap_or_else(|| "toggle".into());
            let resulting = apply_config_change(state, |config| {
                let current = config.ui.empty_prompt_predictions;
                let next = match requested.as_str() {
                    "on" | "1" => true,
                    "off" | "0" | "hide" => false,
                    _ => !current,
                };
                config.ui.empty_prompt_predictions = next;
                next
            });
            ok(&[bool_word(resulting).to_owned()])
        }
        Some("reload") => match Config::load() {
            Ok(fresh) => {
                *state.config.lock().unwrap_or_else(|p| p.into_inner()) = fresh;
                ok(&[])
            }
            Err(e) => vec![format!("err\t{id}\t{}", escape_field(&format!("{e:#}")))],
        },
        Some(op) => vec![format!("err\t{id}\tunknown op {}", escape_field(op))],
        None => vec![format!("err\t{id}\tempty request")],
    }
}

fn bool_word(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn mutate_history(
    id: &str,
    state: &ServerState,
    mutate: impl FnOnce(&mut HistoryDb) -> bool,
) -> Vec<String> {
    // The whole read-modify-write cycle happens under the write lock, and
    // starts from the freshest on-disk state: a direct CLI invocation may
    // have recorded habits since our last mutation, and persisting a stale
    // in-memory snapshot would revert them. Holding the lock across the
    // cycle also makes concurrent record/feedback requests serialize
    // instead of racing load-against-load (a lost update).
    let mut guard = state.history.write().unwrap_or_else(|p| p.into_inner());
    let mut latest = HistoryDb::load_or_fresh();
    let changed = mutate(&mut latest);
    if changed {
        latest.prune();
        // Persistence failure must not poison the in-memory model.
        let _ = latest.save();
    }
    *guard = std::sync::Arc::new(latest);
    drop(guard);
    vec![format!("ok\t{id}")]
}

fn apply_config_change<R>(state: &ServerState, change: impl FnOnce(&mut Config) -> R) -> R {
    let mut config = state.config.lock().unwrap_or_else(|p| p.into_inner());
    let result = change(&mut config);
    let _ = config.save();
    result
}

fn handle_query(fields: &[String], state: &ServerState) -> Result<Vec<String>> {
    let list = fields.get(2).map(|v| v == "1").unwrap_or(false);
    let json = fields.get(7).map(|v| v == "json").unwrap_or(false);
    let offline = fields.get(8).map(|v| v == "1").unwrap_or(false);
    let buffer = fields.get(3).cloned().unwrap_or_default();
    let cursor_chars: usize = fields
        .get(4)
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| buffer.chars().count());
    // The CLI protocol measures the cursor in characters; the engine wants
    // a byte offset, so translate exactly like the one-shot path does.
    let cursor_bytes = buffer
        .char_indices()
        .nth(cursor_chars)
        .map_or(buffer.len(), |(index, _)| index);
    let cwd = fields
        .get(5)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let prev = fields.get(6).filter(|v| !v.is_empty()).map(String::as_str);
    let id = fields.get(1).cloned().unwrap_or_else(|| "0".into());

    // Snapshot the profile and release the lock before any engine work:
    // discovery can spend seconds in subprocesses, and queries must never
    // delay a concurrent record or feedback write.
    let profile = std::sync::Arc::clone(&state.history.read().unwrap_or_else(|p| p.into_inner()));
    let config = state
        .config
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let engine = Engine::new(state.cache.clone(), config);
    let request = crate::engine::QueryRequest {
        buffer: &buffer,
        cursor: cursor_bytes,
        cwd: &cwd,
        offline,
        prev,
        profile: &profile,
    };
    let response = if list {
        engine.list(request)?
    } else {
        engine.query(request)?
    };
    Ok(render_response(&id, &response, json))
}

/// Serializes a response either as the sanitized TSV layout the zsh frontend
/// parses (`res` header plus `cand` rows) or as one compact JSON line for
/// programmatic clients. Both carry the echoed request id.
fn render_response(id: &str, r: &QueryResponse, json: bool) -> Vec<String> {
    if json {
        return vec![format!(
            "jres\t{}\t{}",
            escape_field(id),
            serde_json::to_string(r).unwrap_or_default()
        )];
    }
    let mut out = vec![format!(
        "res\t{}\t{}\t{}\t{}\t{}",
        escape_field(id),
        r.request_id,
        r.prefix_len,
        r.candidates.len(),
        escape_field(&r.context_key)
    )];
    // Rows use exactly the four-column layout of the CLI's `--format zsh`
    // output so every consumer parses one row shape.
    for c in &r.candidates {
        out.push(format!(
            "{}\t{}\t{}\t{}",
            escape_field(&clean(&c.value)),
            escape_field(&clean(&c.display)),
            escape_field(&clean(&c.description)),
            serde_json::to_value(c.source)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default()
        ));
    }
    out
}

/// Same output filter the CLI applies before printing candidate fields --
/// byte-identical to it on purpose: both transports must hand the frontend
/// exactly the same text (shell-escaped values like `space\ dir/` keep
/// their backslashes), and framing safety comes from [`escape_field`],
/// applied afterwards.
fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && *c != '\u{1b}')
        .take(1024)
        .collect()
}

// ---- client -----------------------------------------------------------------

pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(60);
pub const QUERY_TIMEOUT: Duration = Duration::from_millis(400);

/// A short-lived connection used for control operations (ping, toggles,
/// record, feedback): open, exchange one line, close. Queries use
/// [`query_daemon`] so they can read multi-line answers.
pub struct DaemonClient {
    stream: UnixStream,
}

impl DaemonClient {
    pub fn connect() -> Result<Self> {
        let path = history::socket_path();
        let addr = unix_addr(&path)?;
        // A filesystem socket is local: connect either succeeds immediately
        // or fails with ENOENT/ECONNREFUSED/eperm -- there is no meaningful
        // dial timeout to manage. Post-connect operations carry real
        // deadlines instead.
        let stream = UnixStream::connect_addr(&addr)
            .with_context(|| format!("could not reach daemon at {}", path.display()))?;
        Ok(Self { stream })
    }

    /// Sends one request, reads exactly one response line back.
    pub fn call(&mut self, fields: &[&str], timeout: Duration) -> Result<Vec<String>> {
        self.stream.set_read_timeout(Some(timeout))?;
        self.stream.set_write_timeout(Some(timeout))?;
        let encoded = fields
            .iter()
            .map(|f| escape_field(f))
            .collect::<Vec<_>>()
            .join("\t");
        writeln!(self.stream, "{encoded}")?;
        self.stream.flush()?;
        let mut line = String::new();
        let mut reader = BufReader::new(&self.stream);
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            anyhow::bail!("daemon closed the connection");
        }
        Ok(split_line(line.trim_end_matches(['\n', '\r'])))
    }
}

fn unix_addr(path: &Path) -> Result<std::os::unix::net::SocketAddr> {
    std::os::unix::net::SocketAddr::from_pathname(path)
        .with_context(|| format!("invalid socket path {}", path.display()))
}

/// Asks the daemon for a completion. The wire answer is always compact JSON;
/// this deserializes it so the CLI can render identically no matter which
/// transport served the request (socket or direct engine run).
pub fn query_daemon(
    list: bool,
    buffer: &str,
    cursor: usize,
    cwd: &str,
    prev: &str,
    offline: bool,
) -> Option<QueryResponse> {
    let mut client = DaemonClient::connect().ok()?;
    client.stream.set_read_timeout(Some(QUERY_TIMEOUT)).ok()?;
    client.stream.set_write_timeout(Some(QUERY_TIMEOUT)).ok()?;
    let encoded = [
        "query",
        "0",
        if list { "1" } else { "0" },
        buffer,
        &cursor.to_string(),
        cwd,
        prev,
        "json",
        if offline { "1" } else { "0" },
    ]
    .iter()
    .map(|f| escape_field(f))
    .collect::<Vec<_>>()
    .join("\t");
    writeln!(client.stream, "{encoded}").ok()?;
    client.stream.flush().ok()?;
    // One reply line per query: `jres<TAB><id><TAB><compact json>` on
    // success, `err<TAB><id><TAB><message>` otherwise.
    let mut reader = BufReader::new(&client.stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let head = split_line(line.trim_end_matches(['\n', '\r']));
    if head.first().map(String::as_str) != Some("jres") {
        return None;
    }
    serde_json::from_str(head.get(2)?).ok()
}

/// Reports whether a healthy daemon owns the socket right now, along with
/// its reported version (empty when unreachable).
pub fn daemon_status() -> (bool, String) {
    match DaemonClient::connect() {
        Ok(mut client) => match client.call(&["ping", "0"], Duration::from_millis(200)) {
            // ok<TAB>id<TAB>pong<TAB>version
            Ok(reply) if reply.first().map(String::as_str) == Some("ok") => (
                true,
                reply.get(3).cloned().unwrap_or_else(|| "unknown".into()),
            ),
            _ => (true, "unresponsive".into()),
        },
        Err(_) => (false, String::new()),
    }
}

/// Best-effort detached spawn of a fresh server, used when no healthy
/// daemon owns the socket. Only acts when the caller opted in
/// (`ADAPTIVE_AUTO_DAEMON=1`, exported by the generated init script) and
/// nobody disabled the feature outright (`ADAPTIVE_NO_DAEMON=1`). Never
/// returns a hard error: the invoking query simply continues on the direct
/// path this once.
pub fn ensure_daemon_running() {
    if std::env::var_os("ADAPTIVE_NO_DAEMON").is_some_and(|v| v == "1") {
        return;
    }
    if std::env::var_os("ADAPTIVE_AUTO_DAEMON").is_none_or(|v| v != "1") {
        return;
    }
    start_detached();
}

/// Spawns a detached `serve` process unconditionally (used by
/// `adaptive daemon start` and the auto-spawn path). Best effort: failures
/// are silent because callers always retain a working fallback.
pub fn start_detached() {
    let sock_path = history::socket_path();
    if sock_path.exists() && DaemonClient::connect().is_ok() {
        return;
    }
    if sock_path.exists() {
        // Stale socket from a crashed daemon.
        let _ = std::fs::remove_file(&sock_path);
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = Command::new(exe)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_codec_roundtrips_hostile_text() {
        for raw in [
            "plain",
            "with\ttab",
            "back\\slash",
            "multi\nline\rreturn",
            "",
            "unicode é漢字",
            "mixed\\\ttypes\r\n\r",
        ] {
            assert_eq!(unescape_field(&escape_field(raw)), raw);
        }
    }

    #[test]
    fn escaping_strips_other_control_characters() {
        assert_eq!(escape_field("a\u{7}b"), "ab");
        assert_eq!(escape_field("\u{1b}[31m"), "[31m");
    }
}
