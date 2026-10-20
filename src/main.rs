use adaptive_completion::{
    VERSION,
    cache::Cache,
    config::{Config, FuzzyPreset},
    daemon::{self, DaemonClient},
    discovery::{Discoverer, schema_namespace},
    engine::Engine,
    history::{self, FeedbackKind, HistoryDb},
    model::QueryResponse,
};
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::{
    env, fs,
    io::BufRead,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, UNIX_EPOCH},
};

#[derive(Parser)]
#[command(
    name = "adaptive",
    version,
    about = "Context-aware shell completion engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Init {
        #[arg(value_enum)]
        shell: Shell,
    },
    Query(QueryArgs),
    /// Run the completion server: one process, one in-memory profile,
    /// answering queries over a Unix socket for every open terminal.
    Serve {
        /// Stop when no request has arrived for this many seconds.
        #[arg(long, default_value_t = 900)]
        idle_exit_secs: u64,
    },
    /// Manage the background completion server.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Show or change how far fuzzy matches may sprawl
    /// (tight -> smart -> loose). Persists globally.
    Fuzzy {
        /// Omit to show the current preset; `cycle`/`back` step the ladder.
        preset: Option<String>,
    },
    /// Show or toggle whole-command prediction on an untouched prompt.
    Empty {
        #[arg(value_enum)]
        state: Option<ToggleState>,
    },
    Discover {
        command: String,
        path: Vec<String>,
        #[arg(long)]
        refresh: bool,
    },
    Inspect {
        command: String,
        path: Vec<String>,
    },
    Doctor,
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
    /// Manage the optional carapace-bin completion source.
    Carapace {
        #[command(subcommand)]
        command: CarapaceCommand,
    },
    History {
        #[command(subcommand)]
        command: HistoryCommand,
    },
    Version,
}
#[derive(Clone, ValueEnum)]
enum ToggleState {
    On,
    Off,
    Toggle,
}
#[derive(Subcommand)]
enum DaemonCommand {
    /// Start the server if none is healthy (detached; returns once it
    /// answers pings).
    Start,
    /// Ask a running server to save state and exit.
    Stop,
    /// Report whether a server is running, and its version.
    Status,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Shell {
    Zsh,
    /// The Readline / Bash-programmable-completion frontend.
    Bash,
    /// ble.sh: a Bash line editor written in pure Bash. Needed for the one
    /// thing Readline cannot do -- a *dimmed* inline suggestion -- because
    /// Readline renders ANSI escapes literally.
    Blesh,
    Fish,
}
#[derive(Args)]
struct QueryArgs {
    #[arg(long)]
    buffer: String,
    #[arg(long)]
    cursor: Option<usize>,
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// The previously executed command line; feeds sequence prediction.
    #[arg(long)]
    prev: Option<String>,
    #[arg(long)]
    offline: bool,
    #[arg(long)]
    list: bool,
    /// Answer from a freshly loaded profile even when a daemon is running
    /// (also honored via ADAPTIVE_NO_DAEMON=1).
    #[arg(long)]
    no_daemon: bool,
    #[arg(long, value_enum, default_value = "json")]
    format: OutputFormat,
}
#[derive(Clone, ValueEnum)]
enum OutputFormat {
    Json,
    Zsh,
}
#[derive(Subcommand)]
enum CacheCommand {
    Status,
    Clear,
    Refresh { command: String, path: Vec<String> },
    Prune,
}
#[derive(Subcommand)]
enum CarapaceCommand {
    /// Show whether carapace is detected, and whether it is enabled.
    Status,
    /// Use carapace as a completion source when it is installed.
    Enable,
    /// Stop consulting carapace without uninstalling anything.
    Disable,
}
#[derive(Subcommand)]
enum HistoryCommand {
    Status,
    Clear,
    Disable,
    Enable,
    Record {
        /// Working directory the command ran in (directory affinity).
        #[arg(long)]
        cwd: Option<String>,
        /// Previously executed line (sequence learning).
        #[arg(long)]
        prev: Option<String>,
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Record explicit accept/reject feedback for a suggestion shown at a
    /// query context (the engine echoes `context_key` with each response).
    Feedback {
        #[arg(value_enum)]
        kind: FeedbackKindArg,
        #[arg(long)]
        context: String,
        #[arg(long)]
        value: String,
    },
    /// Backfill the usage profile from a shell history file (default
    /// $HISTFILE). Runs once unless --force is given.
    Import {
        file: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
}
#[derive(Clone, ValueEnum)]
enum FeedbackKindArg {
    Accept,
    Reject,
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("adaptive: {e:#}");
            ExitCode::FAILURE
        }
    }
}
fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { shell: Shell::Zsh } => {
            let target = write_cached_init()?;
            println!("source {}", shell_quote(&target));
        }
        Command::Init { shell: Shell::Bash } => {
            let target = write_cached_bash_init(Shell::Bash)?;
            println!("source {}", shell_quote(&target));
        }
        Command::Init {
            shell: Shell::Blesh,
        } => {
            let target = write_cached_bash_init(Shell::Blesh)?;
            println!("source {}", shell_quote(&target));
        }
        Command::Init { shell: Shell::Fish } => {
            anyhow::bail!("the Fish frontend is planned; track it in the README roadmap")
        }
        Command::Serve { idle_exit_secs } => {
            daemon::serve(Duration::from_secs(idle_exit_secs))?;
        }
        Command::Daemon { command } => daemon_command(command)?,
        Command::Fuzzy { preset } => fuzzy_command(preset.as_deref())?,
        Command::Empty { state } => empty_command(state)?,
        Command::Query(q) => query_command(q)?,
        Command::Discover {
            command,
            path,
            refresh,
        } => {
            let c = Config::load()?;
            let cache = Cache::new();
            let s = Discoverer::new(&cache, c.completion.online_docs)
                .discover(&command, &path, refresh)?;
            println!("{}", serde_json::to_string_pretty(&s)?)
        }
        Command::Inspect { command, path } => {
            let cache = Cache::new();
            let key = format!("{command}:{}", path.join("/"));
            if let Some::<adaptive_completion::model::CommandSchema>(s) =
                cache.get(&schema_namespace(), &key)?
            {
                println!("{}", serde_json::to_string_pretty(&s)?)
            } else {
                anyhow::bail!("no cached schema; run `adaptive discover {command}`")
            }
        }
        Command::Doctor => doctor()?,
        Command::Cache { command } => cache_command(command)?,
        Command::Carapace { command } => carapace_command(command)?,
        Command::History { command } => history_command(command)?,
        Command::Version => println!("adaptive {VERSION}"),
    }
    Ok(())
}
fn print_zsh(r: &QueryResponse) {
    println!(
        "{}\t{}\t{}\t{}",
        r.request_id,
        r.prefix_len,
        r.candidates.len(),
        clean(&r.context_key)
    );
    for c in &r.candidates {
        println!(
            "{}\t{}\t{}\t{}",
            clean(&c.value),
            clean(&c.display),
            clean(&c.description),
            serde_json::to_value(c.source)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default()
        );
    }
}
fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && *c != '\u{1b}')
        .take(1024)
        .collect()
}

/// Answers a completion request: through the running server when one is
/// healthy (a sub-millisecond socket round trip), otherwise by loading the
/// profile directly -- spawning the server for next time when allowed. Both
/// transports yield the same `QueryResponse`, so output formatting happens
/// in exactly one place.
fn query_command(q: QueryArgs) -> Result<()> {
    let cwd = q.cwd.clone().unwrap_or(env::current_dir()?);
    let cursor_chars = q.cursor.unwrap_or_else(|| q.buffer.chars().count());
    let daemon_allowed = !q.no_daemon && env::var_os("ADAPTIVE_NO_DAEMON").is_none_or(|v| v != "1");
    let mut served = None;
    if daemon_allowed
        && let Some(response) = daemon::query_daemon(
            q.list,
            &q.buffer,
            cursor_chars,
            &cwd.to_string_lossy(),
            q.prev.as_deref().unwrap_or(""),
            q.offline,
        )
    {
        served = Some(response);
    } else if daemon_allowed {
        // Nothing healthy answered: leave a fresh server behind for the next
        // keystroke (no-op unless this shell opted in via ADAPTIVE_AUTO_DAEMON).
        daemon::ensure_daemon_running();
    }
    let response = match served {
        Some(response) => response,
        None => {
            let config = Config::load()?;
            let engine = Engine::new(Cache::new(), config);
            let cursor_bytes = q
                .buffer
                .char_indices()
                .nth(cursor_chars)
                .map_or(q.buffer.len(), |(index, _)| index);
            let profile = HistoryDb::load_or_fresh();
            let request = adaptive_completion::engine::QueryRequest {
                buffer: &q.buffer,
                cursor: cursor_bytes,
                cwd: &cwd,
                offline: q.offline,
                prev: q.prev.as_deref(),
                profile: &profile,
            };
            if q.list {
                engine.list(request)?
            } else {
                engine.query(request)?
            }
        }
    };
    match q.format {
        OutputFormat::Json => println!("{}", serde_json::to_string(&response)?),
        OutputFormat::Zsh => print_zsh(&response),
    }
    Ok(())
}

/// Writes the generated integration to `<data dir>/init.zsh` (atomically,
/// only when its content changed) so shells can `source` it without ever
/// launching this binary. Returns the file's path.
fn write_cached_init() -> Result<PathBuf> {
    let dir = history::data_dir();
    history::ensure_private_dir(&dir);
    let target = dir.join("init.zsh");
    let exe = env::current_exe().unwrap_or_else(|_| PathBuf::from("adaptive"));
    let (size, mtime) = binary_identity(&exe);
    // Shell startup sources whatever this prints, so it must succeed even
    // when the saved config is broken: fall back to the default instead of
    // propagating the error.
    let ghost_preamble = format!(
        ": ${{ADAPTIVE_GHOST_TEXT:={}}}",
        u8::from(Config::load().map(|c| c.ui.ghost_text).unwrap_or(true))
    );
    let body = format!(
        "# Generated by `adaptive init zsh`; do not edit -- regenerate instead.\n\
         typeset -g ADAPTIVE_BIN_PATH={path}\n\
         typeset -g ADAPTIVE_BIN_SIZE={size}\n\
         typeset -g ADAPTIVE_BIN_MTIME={mtime}\n\
         typeset -g ADAPTIVE_DEFAULT_SOCK={sock}\n\
         {preamble}\n\
         {script}",
        path = shell_quote(&exe),
        size = size,
        mtime = mtime,
        sock = shell_quote(&history::socket_path()),
        preamble = ghost_preamble,
        script = include_str!("../shell/adaptive.zsh"),
    );
    if fs::read_to_string(&target).is_ok_and(|existing| existing == body) {
        return Ok(target);
    }
    write_if_changed(&target, &body)?;
    Ok(target)
}

/// Writes the generated Bash integration: the shared runtime
/// (`adaptive-core.bash`) plus the frontend for the requested line editor
/// (`init.bash` for GNU Readline, `init.blesh` for ble.sh).
///
/// Both files are written atomically and only when their content changed, so
/// repeated `adaptive init bash` calls cost one read each. The frontends are
/// plain Bash, so the generated file is what a shell sources -- it never has
/// to launch this binary at startup.
fn write_cached_bash_init(shell: Shell) -> Result<PathBuf> {
    let dir = history::data_dir();
    history::ensure_private_dir(&dir);
    let core = dir.join("adaptive-core.bash");
    write_if_changed(&core, include_str!("../shell/adaptive-core.bash"))?;

    let (name, script) = match shell {
        Shell::Bash => ("init.bash", include_str!("../shell/adaptive.bash")),
        Shell::Blesh => ("init.blesh", include_str!("../shell/adaptive.blesh")),
        _ => anyhow::bail!("{shell:?} has no Bash frontend"),
    };
    let target = dir.join(name);
    let exe = env::current_exe().unwrap_or_else(|_| PathBuf::from("adaptive"));
    let (size, mtime) = binary_identity(&exe);
    // Same contract as the Zsh frontend: a broken saved config must not stop
    // a shell from starting, so fall back to the default.
    let ghost_preamble = format!(
        ": ${{ADAPTIVE_GHOST_TEXT:={}}}",
        u8::from(Config::load().map(|c| c.ui.ghost_text).unwrap_or(true))
    );
    let body = format!(
        "# Generated by `adaptive init {shell}`; do not edit -- regenerate instead.\n\
         ADAPTIVE_BIN_PATH={path}\n\
         ADAPTIVE_BIN_SIZE={size}\n\
         ADAPTIVE_BIN_MTIME={mtime}\n\
         ADAPTIVE_DEFAULT_SOCK={sock}\n\
         ADAPTIVE_CORE_PATH={core}\n\
         {preamble}\n\
         [[ -r \"$ADAPTIVE_CORE_PATH\" ]] && source \"$ADAPTIVE_CORE_PATH\"\n\
         {script}",
        shell = match shell {
            Shell::Bash => "bash",
            Shell::Blesh => "blesh",
            _ => "bash",
        },
        path = shell_quote(&exe),
        size = size,
        mtime = mtime,
        sock = shell_quote(&history::socket_path()),
        core = shell_quote(&core),
        preamble = ghost_preamble,
        script = script,
    );
    write_if_changed(&target, &body)?;
    Ok(target)
}

/// Writes `content` to `target` atomically, and only when it differs from
/// what is already there. The file ends up owner-readable only.
fn write_if_changed(target: &Path, content: &str) -> Result<()> {
    if fs::read_to_string(target).is_ok_and(|existing| existing == content) {
        return Ok(());
    }
    let temp = target.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&temp, content).with_context(|| format!("could not write {}", temp.display()))?;
    fs::rename(&temp, target).with_context(|| format!("could not update {}", target.display()))?;
    let _ = fs::set_permissions(target, fs::Permissions::from_mode(0o600));
    Ok(())
}

/// Size and modification time (whole seconds, matching zsh's zstat output)
/// identifying the installed binary; the init script compares them cheaply
/// at source time to notice upgrades.
fn binary_identity(exe: &Path) -> (u64, u64) {
    match fs::metadata(exe) {
        Ok(meta) => (
            meta.len(),
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
        ),
        Err(_) => (0, 0),
    }
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

fn daemon_command(command: DaemonCommand) -> Result<()> {
    match command {
        DaemonCommand::Start => {
            if daemon::daemon_status().0 {
                println!("Daemon already running");
                return Ok(());
            }
            daemon::start_detached();
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                if let (_, version) = daemon::daemon_status()
                    && !version.is_empty()
                {
                    println!("Daemon started ({version})");
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            anyhow::bail!("daemon did not answer within 2s; check `adaptive doctor`")
        }
        DaemonCommand::Stop => {
            let mut client =
                DaemonClient::connect().context("no daemon is running (nothing to stop)")?;
            client.call(&["shutdown", "0"], Duration::from_millis(500))?;
            println!("Daemon stopped");
            Ok(())
        }
        DaemonCommand::Status => {
            let (alive, version) = daemon::daemon_status();
            if !alive {
                println!("Daemon: not running (queries fall back to direct mode)");
            } else if version == VERSION {
                println!("Daemon: running ({version})");
            } else {
                println!(
                    "Daemon: running stale version '{version}' (this binary is {VERSION}); run: adaptive daemon restart"
                );
            }
            Ok(())
        }
    }
}

fn fuzzy_command(preset: Option<&str>) -> Result<()> {
    fn ladder_line(active: FuzzyPreset) -> String {
        let names = ["tight", "smart", "loose"];
        names
            .iter()
            .map(|name| {
                if *name == active.as_str() {
                    format!("*{name}*")
                } else {
                    (*name).to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("   ")
    }
    let Some(requested) = preset else {
        let config = Config::load()?;
        println!(
            "fuzzy {}",
            if config.completion.fuzzy {
                ladder_line(config.fuzzy_preset())
            } else {
                "disabled (completion.fuzzy = false)".to_owned()
            }
        );
        println!("Presets bound how far typed letters may sprawl:");
        println!("  tight  near-adjacent letters      gco -> g.co, gc.o");
        println!("  smart  letters stay close         gco -> git checkout main");
        println!("  loose  letters may wander far     gco -> git checkout -- README");
        println!("Change: adaptive fuzzy <tight|smart|loose|cycle|back>");
        return Ok(());
    };
    let mut config = Config::load()?;
    let next = match requested.trim() {
        "cycle" => config.fuzzy_preset().next(),
        "back" => config.fuzzy_preset().previous(),
        other => FuzzyPreset::parse(other)
            .context("unknown preset; expected tight, smart, loose, cycle, or back")?,
    };
    config.completion.fuzzy = true;
    config.completion.fuzzy_preset = next.as_str().to_owned();
    config.save()?;
    nudge_daemon_reload();
    println!("fuzzy {}", ladder_line(next));
    Ok(())
}

fn empty_command(state: Option<ToggleState>) -> Result<()> {
    let mut config = Config::load()?;
    let next = match state.unwrap_or(ToggleState::Toggle) {
        ToggleState::On => true,
        ToggleState::Off => false,
        ToggleState::Toggle => !config.ui.empty_prompt_predictions,
    };
    config.ui.empty_prompt_predictions = next;
    config.save()?;
    nudge_daemon_reload();
    println!(
        "empty-prompt predictions {}",
        if next { "on" } else { "off" }
    );
    Ok(())
}

/// Best-effort poke so an already-running server picks up a config change
/// written straight to disk by this process.
fn nudge_daemon_reload() {
    if let Ok(mut client) = DaemonClient::connect() {
        let _ = client.call(&["reload", "0"], Duration::from_millis(200));
    }
}

fn cache_command(c: CacheCommand) -> Result<()> {
    let cache = Cache::new();
    match c {
        CacheCommand::Status => {
            let s = cache.status()?;
            println!(
                "Cache: {}\nEntries: {}\nSize: {} bytes",
                s.path.display(),
                s.files,
                s.bytes
            )
        }
        CacheCommand::Clear => {
            cache.clear()?;
            println!("Cache cleared")
        }
        CacheCommand::Refresh { command, path } => {
            let config = Config::load()?;
            Discoverer::new(&cache, config.completion.online_docs)
                .discover(&command, &path, true)?;
            println!("Refreshed {command}")
        }
        CacheCommand::Prune => println!(
            "Removed {} cache entries",
            cache.prune(Duration::from_secs(30 * 86400), 100 * 1024 * 1024)?
        ),
    }
    Ok(())
}
fn carapace_command(command: CarapaceCommand) -> Result<()> {
    use adaptive_completion::carapace::Carapace;
    match command {
        CarapaceCommand::Status => {
            let config = Config::load()?;
            let detected = Carapace::detect();
            match &detected {
                Some(carapace) => println!(
                    "Detected: {} ({})",
                    carapace.path().display(),
                    carapace.version()
                ),
                None => println!("Detected: not found on PATH"),
            }
            println!("Setting: carapace.enabled = {}", config.carapace.enabled);
            let state = match (&detected, config.carapace.enabled) {
                (Some(_), true) => "active",
                (Some(_), false) => "inactive (disabled in config)",
                (None, true) => "unavailable (install carapace-bin to enable)",
                (None, false) => "unavailable",
            };
            println!("State: {state}");
        }
        CarapaceCommand::Enable | CarapaceCommand::Disable => {
            let enabling = matches!(command, CarapaceCommand::Enable);
            let mut config = Config::load()?;
            config.carapace.enabled = enabling;
            config.save()?;
            println!("carapace.enabled = {enabling}");
            if Carapace::detect().is_none() {
                println!(
                    "carapace was not found on PATH; install carapace-bin to activate this source"
                );
            } else if enabling {
                println!("carapace answers are now merged into completions");
            }
        }
    }
    Ok(())
}

fn history_command(c: HistoryCommand) -> Result<()> {
    // Lenient by design: a corrupt profile must never wedge the learning
    // commands behind a manual file deletion -- the next save rewrites it.
    let mut h = HistoryDb::load_or_fresh();
    match c {
        HistoryCommand::Status => {
            println!(
                "History: {}\nPatterns: {}\nTransitions: {} command paths\nSequences: {} previous commands\nDirectories: {}\nFeedback contexts: {} accepted / {} rejected\nStorage: local only",
                if h.enabled { "enabled" } else { "disabled" },
                h.patterns.len(),
                h.transitions.len(),
                h.sequences.len(),
                h.dirs.len(),
                h.accepted.len(),
                h.rejected.len()
            );
        }
        HistoryCommand::Clear => {
            h.clear();
            h.save()?;
            let _ = fs::remove_file(history::import_marker_path());
            println!("History cleared")
        }
        HistoryCommand::Disable => {
            h.enabled = false;
            h.save()?;
            println!("History disabled")
        }
        HistoryCommand::Enable => {
            h.enabled = true;
            h.save()?;
            println!("History enabled")
        }
        HistoryCommand::Record { cwd, prev, command } => {
            if h.record(&command.join(" "), cwd.as_deref(), prev.as_deref()) {
                h.prune();
                h.save()?
            }
        }
        HistoryCommand::Feedback {
            kind,
            context,
            value,
        } => {
            let kind = match kind {
                FeedbackKindArg::Accept => FeedbackKind::Accept,
                FeedbackKindArg::Reject => FeedbackKind::Reject,
            };
            if h.feedback(kind, &context, &value) {
                h.prune();
                h.save()?
            }
        }
        HistoryCommand::Import { file, force } => history_import(h, file, force)?,
    }
    Ok(())
}

fn history_import(mut h: HistoryDb, file: Option<PathBuf>, force: bool) -> Result<()> {
    let marker = history::import_marker_path();
    if !force && marker.exists() {
        println!("Already imported (use --force to re-import)");
        return Ok(());
    }
    let path = file
        .or_else(|| env::var_os("HISTFILE").map(PathBuf::from))
        .context("no history file given and $HISTFILE is not set")?;
    let input =
        fs::File::open(&path).with_context(|| format!("could not read {}", path.display()))?;
    let imported = h.import_lines(
        std::io::BufReader::new(input)
            .lines()
            .map_while(std::result::Result::ok),
    );
    h.prune();
    h.save()?;
    if !marker.exists() && fs::write(&marker, "{}").is_err() {
        // Marker is best-effort; a failed write only risks double import.
    }
    println!("Imported {imported} lines from {}", path.display());
    Ok(())
}
fn doctor() -> Result<()> {
    let config = Config::load().context("configuration check failed")?;
    println!("Adaptive {VERSION}");
    println!(
        "Configuration: {} ({})",
        Config::path().display(),
        if Config::path().exists() {
            "loaded"
        } else {
            "defaults"
        }
    );
    let s = Cache::new().status()?;
    println!(
        "Cache: writable at {} ({} entries)",
        s.path.display(),
        s.files
    );
    let (alive, daemon_version) = daemon::daemon_status();
    if !alive {
        println!("Daemon: not running (queries fall back to the direct path)");
    } else if daemon_version == VERSION {
        println!("Daemon: running ({daemon_version})");
    } else {
        println!(
            "Daemon: running stale version '{daemon_version}' (binary is {VERSION}); run: adaptive daemon restart"
        );
    }
    println!("Git: {}", available("git"));
    println!("GitHub CLI: {}", available("gh"));
    println!(
        "Carapace: {} ({})",
        match adaptive_completion::carapace::Carapace::detect() {
            Some(carapace) => carapace.path().display().to_string(),
            None => "not installed".into(),
        },
        if config.carapace.enabled {
            "optional source enabled"
        } else {
            "disabled in config"
        }
    );
    println!("Forge providers: github.com, gitlab.com, codeberg.org, gitea.com, bitbucket.org");
    println!("Zsh: {}", available("zsh"));
    println!(
        "Online docs: {}",
        if config.completion.online_docs {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!("Telemetry: disabled (not implemented)");
    let zshrc = dirs::home_dir().map(|p| p.join(".zshrc"));
    let integrated = zshrc.as_deref().is_some_and(|p| {
        std::fs::read_to_string(p).is_ok_and(|s| s.contains("adaptive initialize"))
    });
    println!(
        "Zsh integration: {}",
        if integrated {
            "managed block found"
        } else {
            "not detected; add the block from install.sh (sources the cached init.zsh directly)"
        }
    );
    let bashrc = dirs::home_dir().map(|p| p.join(".bashrc"));
    let bash_integrated = bashrc.as_deref().is_some_and(|p| {
        std::fs::read_to_string(p).is_ok_and(|s| s.contains("adaptive init bash"))
    });
    println!(
        "Bash integration: {}",
        if bash_integrated {
            "detected"
        } else {
            "not detected; add `eval \"$(adaptive init bash)\"` (requires bash >= 4)"
        }
    );
    println!("Bash line editor: {}", bash_line_editor());
    Ok(())
}

/// Which line editor a Bash session will use, and therefore how Adaptive's
/// inline suggestion can be rendered: ble.sh paints styled regions, so the
/// suggestion can be dimmed; GNU Readline renders ANSI escapes literally, so
/// the suggestion is plain buffer text instead.
fn bash_line_editor() -> &'static str {
    if blesh_installed() {
        "ble.sh (dimmed inline suggestions)"
    } else {
        "GNU Readline (inline suggestions appear as plain buffer text)"
    }
}

/// Whether ble.sh is installed in one of the usual places.
fn blesh_installed() -> bool {
    let data = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
    data.join("blesh/ble.sh").exists()
        || dirs::home_dir().is_some_and(|h| h.join(".blesh/ble.sh").exists())
}
fn available(name: &str) -> &'static str {
    if env::var_os("PATH")
        .and_then(|p| {
            env::split_paths(&p)
                .map(|d| d.join(name))
                .find(|p| p.is_file())
        })
        .is_some()
    {
        "available"
    } else {
        "not found"
    }
}
