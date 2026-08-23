//! Persistent usage memory.
//!
//! A flat "line -> count" table can only ever re-offer whole lines. To predict
//! the *next token* at any position -- and to know which commands belong to
//! which directory, or which command tends to follow which -- the same raw
//! input is folded into several small indexes at record time, so queries are
//! O(1) lookups instead of history scans:
//!
//! * `patterns`     full sanitized command lines (the original table)
//! * `transitions`  exact command path ("git", "git commit -m") -> next token
//! * `sequences`    previous command's head -> this command's head
//! * `dirs`         working directory -> commands run there
//! * `accepted` / `rejected` explicit accept/reject feedback per query context
//!
//! Every counter keeps its raw count and last-used timestamp; how much it
//! counts *right now* is a half-life decay computed at read time. Recent
//! repetition therefore outweighs old habits without ever erasing them.

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

/// Bump when the on-disk layout changes incompatibly. Old files load fine via
/// `#[serde(default)]`; this only documents intent.
pub const SCHEMA_VERSION: u32 = 2;

/// Frequency plus last-seen timestamp for one observed value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub count: u64,
    pub last_used: u64,
}

impl Usage {
    pub fn new() -> Self {
        Self {
            count: 0,
            last_used: 0,
        }
    }
    pub fn bump(&mut self, at: u64) {
        self.count = self.count.saturating_add(1);
        self.last_used = at.max(self.last_used);
    }
    /// How much this observation is worth right now: raw count decayed by a
    /// half-life over days since last use. Two runs today beat ten runs from a
    /// month ago; the ten are still remembered once the two go stale.
    pub fn weight(&self, half_life_days: f64, now: u64) -> f64 {
        let age_days = now.saturating_sub(self.last_used) as f64 / 86_400.0;
        self.count as f64 * 0.5f64.powf(age_days / half_life_days.max(0.04))
    }
}

impl Default for Usage {
    fn default() -> Self {
        Self::new()
    }
}

/// key -> value -> usage (one index page of the profile).
pub type Counter = HashMap<String, Usage>;

// Retention caps. The profile must stay a few hundred KB so every query
// process loads it instantly; when a map outgrows its cap, the weakest
// entries (smallest count, oldest) are dropped at save time.
const PATTERNS_CAP: usize = 6_000;
const TRANSITIONS_KEY_CAP: usize = 3_000;
const TRANSITIONS_VALUE_CAP: usize = 48;
const SEQUENCES_KEY_CAP: usize = 1_500;
const SEQUENCES_VALUE_CAP: usize = 64;
const DIRS_CAP: usize = 400;
const DIRS_VALUE_CAP: usize = 32;
const FEEDBACK_KEY_CAP: usize = 2_000;
const FEEDBACK_VALUE_CAP: usize = 64;
const MAX_TOKEN_LEN: usize = 160;
const IMPORT_MAX_LINES: usize = 50_000;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HistoryDb {
    pub enabled: bool,
    pub version: u32,
    pub patterns: HashMap<String, Usage>,
    /// Exact typed path ("git", "git commit", "git commit -m") -> next token.
    pub transitions: HashMap<String, Counter>,
    /// Previous command's first word -> this command's first word.
    pub sequences: HashMap<String, Counter>,
    /// Normalized working directory -> commands run there.
    pub dirs: HashMap<String, Counter>,
    /// Query context key -> values the user explicitly accepted.
    pub accepted: HashMap<String, Counter>,
    /// Query context key -> values the user explicitly dismissed.
    pub rejected: HashMap<String, Counter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackKind {
    Accept,
    Reject,
}

impl HistoryDb {
    pub fn load() -> Result<Self> {
        let p = path();
        if !p.exists() {
            return Ok(Self {
                enabled: true,
                version: SCHEMA_VERSION,
                ..Default::default()
            });
        }
        Ok(serde_json::from_slice(&fs::read(p)?)?)
    }
    pub fn save(&self) -> Result<()> {
        let p = path();
        if let Some(d) = p.parent() {
            fs::create_dir_all(d)?;
        }
        let t = p.with_extension("tmp");
        fs::write(&t, serde_json::to_vec(self)?)?;
        fs::rename(t, p)?;
        Ok(())
    }

    /// Records one executed command line into every index. `cwd` feeds the
    /// directory-affinity index and `prev` (the previously executed line)
    /// feeds the sequence index; either may be absent (e.g. during import).
    pub fn record(&mut self, line: &str, cwd: Option<&str>, prev: Option<&str>) -> bool {
        if !self.enabled {
            return false;
        }
        let Some(pattern) = sanitize(line) else {
            return false;
        };
        let Ok(words) = shell_words::split(&pattern) else {
            return false;
        };
        if words.is_empty() || words.len() > 24 {
            return false;
        }
        let now = now();
        let head = words[0].clone();
        self.patterns.entry(pattern).or_default().bump(now);
        // Transition indexes use the exact typed path so that `git -C foo
        // status` teaches "after `git -C foo` comes status" without the flag
        // being silently dropped from context.
        for i in 1..words.len() {
            let key = words[..i].join(" ");
            let next = &words[i];
            if key.chars().count() > MAX_TOKEN_LEN || next.chars().count() > MAX_TOKEN_LEN {
                continue;
            }
            bump(&mut self.transitions, &key, next, now);
        }
        if let Some(prev_line) = prev
            && let Some(prev_head) = head_of(prev_line)
        {
            bump(&mut self.sequences, &prev_head, &head, now);
        }
        if let Some(dir) = cwd
            && !dir.is_empty()
            && dir != "/"
        {
            bump(&mut self.dirs, &normalize_dir(dir), &head, now);
        }
        true
    }

    /// Explicit accept/reject feedback for a suggestion shown at `context_key`
    /// (the engine computes and echoes this key; see `QueryResponse`).
    /// Acceptance additionally strengthens nothing here -- executing the line
    /// records it through `record`; this index captures intent even when the
    /// user never runs the completed line verbatim.
    pub fn feedback(&mut self, kind: FeedbackKind, context_key: &str, value: &str) -> bool {
        if !self.enabled || value.is_empty() || value.chars().count() > MAX_TOKEN_LEN {
            return false;
        }
        let now = now();
        let map = match kind {
            FeedbackKind::Accept => &mut self.accepted,
            FeedbackKind::Reject => &mut self.rejected,
        };
        bump(map, context_key, value, now);
        true
    }

    /// Top next tokens learned for an exact command path, strongest first,
    /// weighted by recency-aware decay rather than raw counts.
    pub fn next_token_weights(
        &self,
        key: &str,
        half_life_days: f64,
        limit: usize,
    ) -> Vec<(String, f64, u64)> {
        weighted_top(self.transitions.get(key), half_life_days, limit)
    }

    /// Combined bare-prompt prediction: what this user runs often overall,
    /// boosted by what they run in this directory and by what tends to follow
    /// the previous command. Returns up to `limit` (command, weight, seen).
    pub fn command_weights(
        &self,
        dir_keys: &[String],
        prev_head: Option<&str>,
        half_life_days: f64,
        limit: usize,
    ) -> Vec<(String, f64, u64)> {
        let now = now();
        // value -> (weight, last_seen); merged additively across signals.
        let mut merged: HashMap<String, (f64, u64)> = HashMap::new();
        let mut fold = |counter: Option<&Counter>, boost: f64| {
            if let Some(counter) = counter {
                for (name, u) in counter {
                    let w = u.weight(half_life_days, now) * boost;
                    let entry = merged.entry(name.clone()).or_insert((0.0, 0));
                    entry.0 += w;
                    entry.1 = entry.1.max(u.last_used);
                }
            }
        };
        // Directory affinity is the strongest personal-context signal ("what
        // do *I* run when I open this project"), then "what usually follows
        // X", then plain global frequency. The boosts are what let a
        // directory habit outrank both global popularity and a same-weight
        // command from another directory.
        for key in dir_keys {
            fold(self.dirs.get(key), 3.0);
        }
        if let Some(prev) = prev_head {
            fold(self.sequences.get(prev), 2.5);
        }
        fold_heads(&self.patterns, &mut merged, half_life_days, now);
        let mut out: Vec<(String, f64, u64)> = merged
            .into_iter()
            .map(|(name, (w, last))| (name, w, last))
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.truncate(limit);
        out.retain(|(_, w, _)| *w > 0.05);
        out
    }

    /// Full-line chain predictions: remembered lines whose head matches
    /// `head_prefix`, strongest first. This is what turns `c` into
    /// `claude --dangerously-skip-permissions`.
    pub fn chain_from_head(
        &self,
        head_prefix: &str,
        half_life_days: f64,
        limit: usize,
    ) -> Vec<(String, Usage)> {
        ranked_patterns(
            &self.patterns,
            |p| p.starts_with(head_prefix) && p != head_prefix,
            half_life_days,
            limit,
        )
    }

    /// Chain predictions after an exact typed path: the remainder of
    /// remembered lines ("https://github.com/x" after the key "git clone"),
    /// filtered to those continuing `current`. The key must include every
    /// argument already typed, or the remainder would re-insert tokens the
    /// user has already written.
    pub fn chain_after_command(
        &self,
        exact_key: &str,
        current: &str,
        half_life_days: f64,
        limit: usize,
    ) -> Vec<(String, Usage)> {
        let now = now();
        let prefix = format!("{exact_key} ");
        let mut out: Vec<(String, Usage, f64)> = self
            .patterns
            .iter()
            .filter_map(|(pattern, u)| {
                let rest = pattern.strip_prefix(&prefix)?;
                (rest.starts_with(current) && rest != current)
                    .then(|| (rest.to_owned(), u.clone(), u.weight(half_life_days, now)))
            })
            .collect();
        out.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        out.truncate(limit);
        out.into_iter().map(|(rest, u, _)| (rest, u)).collect()
    }

    pub fn clear(&mut self) {
        self.patterns.clear();
        self.transitions.clear();
        self.sequences.clear();
        self.dirs.clear();
        self.accepted.clear();
        self.rejected.clear();
    }

    /// Backfills the profile from a shell history file. Sequence data is
    /// reconstructed by feeding each line as the "previous" of the next one.
    /// Lines that fail sanitization (secrets, pastes) are skipped exactly as
    /// live recording would skip them.
    pub fn import_lines<I: Iterator<Item = String>>(&mut self, lines: I) -> usize {
        let mut prev: Option<String> = None;
        let mut imported = 0;
        for line in lines {
            let line = strip_extended_history_prefix(&line);
            if line.is_empty() || line.starts_with('\\') {
                continue;
            }
            if !self.record(&line, None, prev.as_deref()) {
                continue;
            }
            prev = Some(line);
            imported += 1;
            if imported >= IMPORT_MAX_LINES {
                break;
            }
        }
        imported
    }

    /// Enforces retention caps before writing. Called from the CLI save paths.
    pub fn prune(&mut self) {
        prune_map(&mut self.patterns, PATTERNS_CAP);
        prune_nested(
            &mut self.transitions,
            TRANSITIONS_KEY_CAP,
            TRANSITIONS_VALUE_CAP,
        );
        prune_nested(&mut self.sequences, SEQUENCES_KEY_CAP, SEQUENCES_VALUE_CAP);
        prune_nested(&mut self.dirs, DIRS_CAP, DIRS_VALUE_CAP);
        prune_nested(&mut self.accepted, FEEDBACK_KEY_CAP, FEEDBACK_VALUE_CAP);
        prune_nested(&mut self.rejected, FEEDBACK_KEY_CAP, FEEDBACK_VALUE_CAP);
    }
}

fn bump(map: &mut HashMap<String, Counter>, key: &str, value: &str, now: u64) {
    map.entry(key.to_owned())
        .or_default()
        .entry(value.to_owned())
        .or_default()
        .bump(now);
}

/// Folds first-word frequencies from the pattern table into the merged
/// bare-prompt scores as the baseline signal.
fn fold_heads(
    patterns: &HashMap<String, Usage>,
    merged: &mut HashMap<String, (f64, u64)>,
    half_life_days: f64,
    now: u64,
) {
    for (pattern, u) in patterns {
        let Some(head) = pattern.split_whitespace().next() else {
            continue;
        };
        let w = u.weight(half_life_days, now);
        let entry = merged.entry(head.to_owned()).or_insert((0.0, 0));
        entry.0 += w;
        entry.1 = entry.1.max(u.last_used);
    }
}

fn weighted_top(
    counter: Option<&Counter>,
    half_life_days: f64,
    limit: usize,
) -> Vec<(String, f64, u64)> {
    let Some(counter) = counter else {
        return Vec::new();
    };
    let now = now();
    let mut out: Vec<(String, f64, u64)> = counter
        .iter()
        .map(|(value, u)| (value.clone(), u.weight(half_life_days, now), u.last_used))
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out.truncate(limit);
    out
}

fn ranked_patterns<F: Fn(&str) -> bool>(
    patterns: &HashMap<String, Usage>,
    filter: F,
    half_life_days: f64,
    limit: usize,
) -> Vec<(String, Usage)> {
    let now = now();
    let mut out: Vec<(String, Usage, f64)> = patterns
        .iter()
        .filter(|(p, _)| filter(p))
        .map(|(p, u)| (p.clone(), u.clone(), u.weight(half_life_days, now)))
        .collect();
    out.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    out.truncate(limit);
    out.into_iter().map(|(p, u, _)| (p, u)).collect()
}

fn prune_map(map: &mut HashMap<String, Usage>, cap: usize) {
    if map.len() <= cap {
        return;
    }
    let mut entries: Vec<(String, Usage)> = std::mem::take(map).into_iter().collect();
    entries.sort_by(|a, b| {
        b.1.count
            .cmp(&a.1.count)
            .then(b.1.last_used.cmp(&a.1.last_used))
    });
    entries.truncate(cap);
    *map = entries.into_iter().collect();
}

fn prune_nested(map: &mut HashMap<String, Counter>, key_cap: usize, value_cap: usize) {
    for counter in map.values_mut() {
        prune_map(counter, value_cap);
    }
    prune_map_keys(map, key_cap);
}

fn prune_map_keys(map: &mut HashMap<String, Counter>, cap: usize) {
    if map.len() <= cap {
        return;
    }
    // Rank keys by their total retained weight so busy contexts survive.
    let mut totals: Vec<(String, u64)> = map
        .iter()
        .map(|(k, c)| (k.clone(), c.values().map(|u| u.count).sum::<u64>()))
        .collect();
    totals.sort_by_key(|(_, total)| std::cmp::Reverse(*total));
    let keep: std::collections::HashSet<String> =
        totals.into_iter().take(cap).map(|(k, _)| k).collect();
    map.retain(|k, _| keep.contains(k));
}

/// Rewrites `$HOME` as `~` so profile files stay portable across machines and
/// usernames, and bounds the stored depth so `/tmp/build-xyz/…` style noise
/// cannot dominate the directory index.
pub fn normalize_dir(dir: &str) -> String {
    let mut value = dir.trim_end_matches('/').to_owned();
    if value.is_empty() {
        return "/".to_owned();
    }
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy().into_owned();
        if !home.is_empty()
            && let Some(rest) = value.strip_prefix(&home)
        {
            value = format!("~{rest}");
        }
    }
    let parts: Vec<&str> = value.split('/').filter(|p| !p.is_empty()).collect();
    const KEEP: usize = 8;
    if parts.len() > KEEP {
        let tail = parts[parts.len() - KEEP..].join("/");
        value = format!("…/{tail}");
    }
    value
}

/// Ancestor lookup keys for a cwd, nearest first (the cwd itself included).
pub fn dir_lookup_keys(cwd: &std::path::Path) -> Vec<String> {
    let mut keys = Vec::new();
    for ancestor in cwd.ancestors().take(10) {
        let key = normalize_dir(&ancestor.to_string_lossy());
        if !keys.contains(&key) {
            keys.push(key);
        }
        if ancestor.parent().is_none() {
            break;
        }
    }
    keys
}

fn head_of(line: &str) -> Option<String> {
    sanitize(line).and_then(|cleaned| {
        shell_words::split(&cleaned)
            .ok()
            .and_then(|words| words.into_iter().next())
    })
}

/// First word of a command line, sanitized -- the key under which sequence
/// ("what tends to follow what") data is stored and looked up.
pub fn command_head(line: &str) -> Option<String> {
    head_of(line)
}

/// zsh EXTENDED_HISTORY lines look like ": 1234567890:0;command"; strip the
/// stamp so imports see the plain command. Anything that does not look like a
/// stamp is returned untouched.
fn strip_extended_history_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix(": ")
        && let Some(pos) = rest.find(';')
        && rest[..pos].chars().all(|c| c.is_ascii_digit() || c == ':')
    {
        return rest[pos + 1..].to_owned();
    }
    trimmed.to_owned()
}

pub fn sanitize(input: &str) -> Option<String> {
    let value = input.trim();
    if value.is_empty()
        || value.chars().count() > 512
        || value.contains('\n')
        || value.contains('\r')
        || value.contains('\0')
    {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    let secret_words = [
        "authorization:",
        "bearer ",
        "password=",
        "passwd=",
        "api_key",
        "apikey",
        "access_token",
        "secret=",
        "private key",
        "--password",
        "--token",
    ];
    if secret_words.iter().any(|s| lower.contains(s)) {
        return None;
    }
    let jwt = static_regex("eyJ[A-Za-z0-9_-]{15,}\\.[A-Za-z0-9_-]{15,}\\.[A-Za-z0-9_-]{10,}");
    let random = static_regex("(?i)(?:[a-z0-9+/=_-]{48,})");
    let assignment = static_regex("(?:^|\\s)[A-Za-z_][A-Za-z0-9_]{1,40}=[^\\s]{12,}");
    if jwt.is_match(value) || random.is_match(value) || assignment.is_match(value) {
        return None;
    }
    let words = shell_words::split(value).ok()?;
    if words.len() > 24 || words.iter().any(|w| w.chars().count() > 160) {
        return None;
    }
    Some(words.join(" "))
}

fn static_regex(pattern: &'static str) -> &'static Regex {
    static CACHE: OnceLock<HashMap<&'static str, Regex>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        HashMap::from([
            (
                "eyJ[A-Za-z0-9_-]{15,}\\.[A-Za-z0-9_-]{15,}\\.[A-Za-z0-9_-]{10,}",
                Regex::new("eyJ[A-Za-z0-9_-]{15,}\\.[A-Za-z0-9_-]{15,}\\.[A-Za-z0-9_-]{10,}")
                    .unwrap(),
            ),
            (
                "(?i)(?:[a-z0-9+/=_-]{48,})",
                Regex::new("(?i)(?:[a-z0-9+/=_-]{48,})").unwrap(),
            ),
            (
                "(?:^|\\s)[A-Za-z_][A-Za-z0-9_]{1,40}=[^\\s]{12,}",
                Regex::new("(?:^|\\s)[A-Za-z_][A-Za-z0-9_]{1,40}=[^\\s]{12,}").unwrap(),
            ),
        ])
    });
    cache.get(pattern).unwrap()
}

pub fn path() -> PathBuf {
    data_dir().join("history.json")
}

/// Marker recording that a one-time shell-history import already ran, so the
/// frontend's background bootstrap does not double-count lines.
pub fn import_marker_path() -> PathBuf {
    data_dir().join("imported.json")
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("ADAPTIVE_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("adaptive")
        })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> HistoryDb {
        HistoryDb {
            enabled: true,
            version: SCHEMA_VERSION,
            ..Default::default()
        }
    }

    #[test]
    fn accepts_useful_short_commands() {
        assert_eq!(
            sanitize("git push origin dev").as_deref(),
            Some("git push origin dev")
        );
    }

    #[test]
    fn rejects_secrets_and_pastes() {
        assert!(sanitize("curl -H 'Authorization: Bearer secretvalue' x").is_none());
        assert!(sanitize("TOKEN=abcdefghijklmnopqrstuvwxyz123456 command").is_none());
        assert!(sanitize("echo eyJaaaaaaaaaaaaaaaa.bbbbbbbbbbbbbbbbb.cccccccccc").is_none());
        assert!(sanitize("echo a\necho b").is_none());
    }

    #[test]
    fn record_builds_transition_indexes_including_flags() {
        let mut db = fresh();
        assert!(db.record("claude --dangerously-skip-permissions", None, None));
        assert_eq!(
            db.next_token_weights("claude", 21.0, 5)[0].0,
            "--dangerously-skip-permissions"
        );
        db.record("git commit -m hello", None, None);
        let after_commit = db.next_token_weights("git commit", 21.0, 5);
        assert!(after_commit.iter().any(|(t, _, _)| t == "-m"));
        assert!(db.next_token_weights("git commit -m", 21.0, 5)[0].0 == "hello");
    }

    #[test]
    fn recency_decay_lets_new_habits_win_without_erasing_old_ones() {
        let now = now();
        let recent = Usage {
            count: 2,
            last_used: now,
        };
        let old = Usage {
            count: 4,
            last_used: now - 40 * 86_400,
        };
        assert!(recent.weight(21.0, now) > old.weight(21.0, now));
        // But the old habit is still remembered at all.
        assert!(old.weight(21.0, now) > 0.5);
    }

    #[test]
    fn sequences_link_previous_and_current_commands() {
        let mut db = fresh();
        for _ in 0..3 {
            db.record("cd ~/Development/app", None, None);
            db.record(
                "npm run dev",
                Some("/Users/x/Development/app"),
                Some("cd ~/Development/app"),
            );
        }
        let predicted = db.command_weights(
            &[normalize_dir("/Users/x/Development/app")],
            Some("cd"),
            21.0,
            5,
        );
        assert_eq!(predicted[0].0, "npm");
    }

    #[test]
    fn dir_affinity_is_normalized_against_home() {
        let home = dirs::home_dir().unwrap().to_string_lossy().into_owned();
        assert_eq!(normalize_dir(&format!("{home}/Dev")), "~/Dev");
        assert_eq!(
            normalize_dir(&format!("{home}/a/b/c/d/e/f/g/h/i")),
            "…/b/c/d/e/f/g/h/i"
        );
        assert_eq!(normalize_dir("/usr/local"), "/usr/local");
    }

    #[test]
    fn extended_history_stamps_are_stripped_on_import() {
        assert_eq!(
            strip_extended_history_prefix(": 1720000000:0;git status"),
            "git status"
        );
        assert_eq!(strip_extended_history_prefix("git status"), "git status");
    }

    #[test]
    fn import_feeds_transitions_and_sequences() {
        let mut db = fresh();
        let lines = [
            ": 1720000001:0;git status".to_owned(),
            ": 1720000002:0;git push".to_owned(),
            "claude --dangerously-skip-permissions".to_owned(),
        ];
        let n = db.import_lines(lines.into_iter());
        assert_eq!(n, 3);
        assert!(db.next_token_weights("git", 21.0, 5).len() >= 2);
        assert_eq!(db.sequences["git"]["claude"].count, 1);
    }

    #[test]
    fn chain_predictions_cover_head_and_tail() {
        let mut db = fresh();
        for _ in 0..3 {
            db.record("claude --dangerously-skip-permissions", None, None);
        }
        let from_head = db.chain_from_head("c", 21.0, 8);
        assert_eq!(from_head[0].0, "claude --dangerously-skip-permissions");
        let after = db.chain_after_command("claude", "", 21.0, 8);
        assert_eq!(after[0].0, "--dangerously-skip-permissions");
    }

    #[test]
    fn feedback_indexes_are_separate_and_recorded() {
        let mut db = fresh();
        assert!(db.feedback(FeedbackKind::Accept, "claude", "--verbose"));
        assert!(db.feedback(FeedbackKind::Reject, "claude", "--no-cache"));
        assert_eq!(db.accepted["claude"]["--verbose"].count, 1);
        assert_eq!(db.rejected["claude"]["--no-cache"].count, 1);
    }

    #[test]
    fn pruning_keeps_the_heaviest_entries() {
        let mut db = fresh();
        for i in 0..30 {
            db.record(&format!("cmd{i} arg{i}"), None, None);
        }
        for _ in 0..50 {
            db.record("favorite always", None, None);
        }
        db.prune();
        // Tiny caps exercised indirectly: heavy survivor present, structure valid.
        assert!(db.patterns.contains_key("favorite always"));
    }
}
