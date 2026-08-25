//! Optional integration with [`carapace-bin`](https://carapace-sh.github.io/carapace-bin/),
//! an external completion engine carrying structured schemas for hundreds of
//! CLI tools.
//!
//! Carapace answers "what is valid at this position" (subcommands, flags,
//! enum values, positional arguments) with exact command knowledge; Adaptive
//! layers personalization on top through its own ranking. Everything here is
//! opportunistic: when the binary is absent, disabled, slow, or failing, the
//! rest of the pipeline runs exactly as before.
//!
//! Wire protocol (the same one carapace uses between its own shells):
//!
//! ```text
//! carapace <completer> export <args...> <current-word>   # JSON export format
//! carapace --list --names                                # known completers
//! ```
//!
//! Responses are cached under a fingerprint of the carapace binary itself, so
//! upgrading carapace invalidates everything it ever answered without any
//! manual cache maintenance.

use crate::{
    cache::Cache,
    model::Candidate,
    safety::{run_informational, sanitize_remote},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

const COMPLETERS_NAMESPACE: &str = "carapace-completers-v1";
const ANSWERS_NAMESPACE: &str = "carapace-answers-v1";
/// A completer listing changes only when carapace itself changes; the
/// TTL is a backstop, the binary fingerprint the real invalidation.
const COMPLETERS_TTL: Duration = Duration::from_secs(24 * 3600);
/// Answers (branches, scripts, schema entries) go stale as the world moves;
/// ten minutes keeps repeats instant while bounding staleness.
const ANSWERS_TTL: Duration = Duration::from_secs(10 * 60);
/// A hung carapace must never stall the completion pipeline past this.
const QUERY_TIMEOUT: Duration = Duration::from_millis(400);
const LIST_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_ANSWER_ARGS: usize = 48;
const MAX_ARG_LEN: usize = 1024;
const MAX_CANDIDATES: usize = 200;

pub struct Carapace {
    executable: PathBuf,
}

impl Carapace {
    /// Finds carapace on PATH. Cheap enough to call on every query; the
    /// answer cache is what keeps the expensive parts rare.
    pub fn detect() -> Option<Self> {
        find_in_path(&env::var_os("PATH").unwrap_or_default(), "carapace")
            .map(|executable| Self { executable })
    }

    pub fn path(&self) -> &Path {
        &self.executable
    }

    /// Stable identity of this installation: path, size, and mtime. Anything
    /// that changes these (upgrade, reinstall) silently invalidates every
    /// cached answer and completer listing.
    pub fn fingerprint(&self) -> Option<String> {
        let metadata = fs::metadata(&self.executable).ok()?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        Some(format!(
            "{:x}",
            Sha256::digest(
                format!(
                    "{}:{}:{}",
                    self.executable.display(),
                    metadata.len(),
                    modified
                )
                .as_bytes()
            )
        ))
    }

    /// Human-readable version line (`carapace-bin X.Y.Z`), for status output
    /// only -- never on the hot path.
    pub fn version(&self) -> String {
        run_informational(
            self.executable.to_string_lossy().as_ref(),
            ["--version"],
            None,
            LIST_TIMEOUT,
        )
        .map(|out| out.lines().next().unwrap_or_default().trim().to_owned())
        .unwrap_or_else(|_| "unknown version".into())
    }

    /// The names of every completer carapace ships, cached per binary
    /// fingerprint. `None` means the listing could not be determined; callers
    /// should then proceed optimistically rather than skipping the source.
    pub fn known_completers(&self, cache: &Cache) -> Option<Vec<String>> {
        let fingerprint = self.fingerprint()?;
        let key = format!("list:{fingerprint}");
        if let Ok(Some(names)) = cache.get::<Vec<String>>(COMPLETERS_NAMESPACE, &key) {
            return Some(names);
        }
        let output = run_informational(
            self.executable.to_string_lossy().as_ref(),
            ["--list", "--names"],
            None,
            LIST_TIMEOUT,
        )
        .ok()?;
        let names: Vec<String> = output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        let _ = cache.put(COMPLETERS_NAMESPACE, &key, &names, Some(COMPLETERS_TTL));
        Some(names)
    }

    /// Completions for `command` with `args` already typed and `current` the
    /// word being completed. Never fails: any problem (missing binary,
    /// timeout, hostile output) degrades to no candidates.
    ///
    /// The hot path is deliberately two filesystem operations: one stat for
    /// the binary fingerprint and one cached-answer read. Everything else --
    /// the completer listing, the subprocess -- runs only on a cache miss,
    /// and unknown completers are negative-cached like any other answer.
    pub fn complete(
        &self,
        cache: &Cache,
        command: &str,
        args: &[String],
        current: &str,
        cwd: &Path,
    ) -> Vec<Candidate> {
        let Some(fingerprint) = self.fingerprint() else {
            return Vec::new();
        };
        // The working directory shapes answers everywhere (`git checkout`
        // inside one repo differs from another), so it belongs in the key.
        let key = format!(
            "{fingerprint}|{command}|{}|{current}|{}",
            args.join("|"),
            cwd.to_string_lossy()
        );
        if let Ok(Some(cached)) = cache.get::<Vec<Candidate>>(ANSWERS_NAMESPACE, &key) {
            return cached;
        }
        // Cache miss: the completer listing (itself cached per fingerprint)
        // decides whether a subprocess is worth spawning at all. When no
        // listing can be had, asking anyway stays the safe default.
        if let Some(names) = self.known_completers(cache)
            && !names.iter().any(|name| name == command)
        {
            // Empty answers are cached too: an unknown completer or a
            // transient failure must not cost a subprocess on every keystroke.
            let _ = cache.put(
                ANSWERS_NAMESPACE,
                &key,
                &Vec::<Candidate>::new(),
                Some(ANSWERS_TTL),
            );
            return Vec::new();
        }
        let mut cli: Vec<String> = Vec::with_capacity(args.len() + 3);
        cli.push(command.to_owned());
        cli.push("export".into());
        cli.extend(args.iter().take(MAX_ANSWER_ARGS).cloned());
        cli.push(current.chars().take(MAX_ARG_LEN).collect());
        let output = run_informational(
            self.executable.to_string_lossy().as_ref(),
            cli,
            Some(cwd),
            QUERY_TIMEOUT,
        )
        .unwrap_or_default();
        let candidates = parse_export(&output);
        let _ = cache.put(ANSWERS_NAMESPACE, &key, &candidates, Some(ANSWERS_TTL));
        candidates
    }
}

/// Completer names are file-like identifiers; anything else typed as the head
/// of the line (paths, URLs, shell syntax) has no carapace meaning.
pub fn valid_completer_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
        && !name.starts_with('.')
}

/// Parses the JSON "export" interchange format into sanitized candidates.
fn parse_export(output: &str) -> Vec<Candidate> {
    let parsed: Result<Export, _> = serde_json::from_str(output);
    let Ok(export) = parsed else {
        return Vec::new();
    };
    export
        .values
        .into_iter()
        .filter_map(|entry| {
            let value = entry.value.trim();
            if value.is_empty() {
                return None;
            }
            let value = sanitize_remote(value)?;
            let description = entry
                .description
                .as_deref()
                .and_then(sanitize_remote)
                .unwrap_or_else(|| "carapace completion".into());
            let mut candidate = Candidate::new(value, description, crate::model::Source::Carapace);
            if let Some(display) = entry.display.as_deref().and_then(sanitize_remote) {
                candidate.display = display;
            }
            Some(candidate)
        })
        .take(MAX_CANDIDATES)
        .collect()
}

#[derive(Deserialize)]
struct Export {
    #[serde(default)]
    values: Vec<ExportValue>,
}

#[derive(Deserialize)]
struct ExportValue {
    value: String,
    #[serde(default)]
    display: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

fn find_in_path(path_var: &std::ffi::OsStr, name: &str) -> Option<PathBuf> {
    env::split_paths(path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;

    const SAMPLE: &str = r#"{
      "version": "v1.4.2",
      "messages": [],
      "nospace": "",
      "usage": "",
      "values": [
        {"value": "main", "display": "main", "description": "branch"},
        {"value": "feature/x", "description": "branch"},
        {"value": "", "description": "empty is dropped"},
        {"value": "evil\nnewline", "description": "unsanitary"}
      ]
    }"#;

    #[test]
    fn parses_export_json_and_sanitizes_values() {
        let candidates = parse_export(SAMPLE);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].value, "main");
        assert_eq!(candidates[0].description, "branch");
        assert_eq!(candidates[0].source, Source::Carapace);
        assert_eq!(candidates[1].value, "feature/x");
        assert_eq!(candidates[1].description, "branch");
    }

    #[test]
    fn garbage_output_yields_no_candidates() {
        assert!(parse_export("").is_empty());
        assert!(parse_export("not json at all").is_empty());
        assert!(parse_export("{\"values\": \"wrong shape\"}").is_empty());
    }

    #[test]
    fn completer_names_are_strictly_validated() {
        assert!(valid_completer_name("git"));
        assert!(valid_completer_name("docker-compose"));
        assert!(valid_completer_name("gh@copilot"));
        assert!(!valid_completer_name(""));
        assert!(!valid_completer_name("./script"));
        assert!(!valid_completer_name("a b"));
        assert!(!valid_completer_name(".hidden"));
    }

    #[test]
    fn finds_executables_on_a_given_path() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_in_path(std::ffi::OsStr::new(""), "carapace").is_none());
        let shim = dir.path().join("carapace");
        fs::write(&shim, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let found = find_in_path(std::ffi::OsStr::new(dir.path()), "carapace").expect("shim found");
        assert_eq!(found, shim);
    }

    #[test]
    fn answers_are_cached_per_fingerprint_and_position() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let cache = Cache::at(temp.path().join("cache"));
        let bin_dir = tempfile::tempdir().unwrap();
        let counter = temp.path().join("count");
        let shim = bin_dir.path().join("carapace");
        fs::write(
            &shim,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--list\" ]; then\n  printf 'tool\\n'\n  exit 0\nfi\nn=$(cat {} 2>/dev/null || echo 0); echo $((n+1)) > {}\nprintf '{{\"values\":[{{\"value\":\"alpha\"}}]}}'\n",
                counter.display(),
                counter.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
        let carapace = Carapace {
            executable: shim.clone(),
        };
        assert_eq!(
            carapace
                .complete(&cache, "tool", &[], "", Path::new("/tmp"))
                .len(),
            1
        );
        // The identical position is served from cache: the backing process
        // never runs a second time.
        assert_eq!(
            carapace
                .complete(&cache, "tool", &[], "", Path::new("/tmp"))
                .len(),
            1
        );
        assert_eq!(fs::read_to_string(&counter).unwrap(), "1\n");
        // A different position misses the cache; this completer always answers,
        // so the count moves to 2 and the fresh answer is distinct.
        let other = carapace.complete(&cache, "tool", &["x".to_owned()], "", Path::new("/tmp"));
        assert_eq!(other.len(), 1);
        assert_eq!(fs::read_to_string(&counter).unwrap(), "2\n");
        // A different working directory is a different key (repo-local answers).
        carapace.complete(&cache, "tool", &[], "", temp.path());
        assert_eq!(fs::read_to_string(&counter).unwrap(), "3\n");
    }
}
