use adaptive_completion::{
    VERSION,
    cache::Cache,
    config::Config,
    discovery::{Discoverer, schema_namespace},
    engine::Engine,
    history::{self, FeedbackKind, HistoryDb},
    model::QueryResponse,
};
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::{env, fs, io::BufRead, path::PathBuf, process::ExitCode, time::Duration};

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
    History {
        #[command(subcommand)]
        command: HistoryCommand,
    },
    Version,
}
#[derive(Clone, ValueEnum)]
enum Shell {
    Zsh,
    Bash,
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
            let config = Config::load()?;
            println!(
                ": ${{ADAPTIVE_GHOST_TEXT:={}}}",
                u8::from(config.ui.ghost_text)
            );
            print!("{}", include_str!("../shell/adaptive.zsh"));
        }
        Command::Init { shell: Shell::Bash } => {
            print!("{}", include_str!("../shell/adaptive.bash"));
        }
        Command::Init { shell: Shell::Fish } => {
            anyhow::bail!("the Fish frontend is planned; track it in the README roadmap")
        }
        Command::Query(q) => {
            let config = Config::load()?;
            let engine = Engine::new(Cache::new(), config);
            let cwd = q.cwd.unwrap_or(env::current_dir()?);
            let cursor_chars = q.cursor.unwrap_or_else(|| q.buffer.chars().count());
            let cursor_bytes = q
                .buffer
                .char_indices()
                .nth(cursor_chars)
                .map_or(q.buffer.len(), |(index, _)| index);
            let response = if q.list {
                engine.list(&q.buffer, cursor_bytes, &cwd, q.offline, q.prev.as_deref())?
            } else {
                engine.query(&q.buffer, cursor_bytes, &cwd, q.offline, q.prev.as_deref())?
            };
            match q.format {
                OutputFormat::Json => println!("{}", serde_json::to_string(&response)?),
                OutputFormat::Zsh => print_zsh(&response),
            }
        }
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
fn history_command(c: HistoryCommand) -> Result<()> {
    let mut h = HistoryDb::load()?;
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
    println!("Git: {}", available("git"));
    println!("GitHub CLI: {}", available("gh"));
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
            "not detected; add `eval \"$(adaptive init zsh)\"`"
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
    Ok(())
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
