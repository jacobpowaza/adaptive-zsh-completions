use crate::{
    cache::Cache,
    config::Config,
    discovery::Discoverer,
    history::{HistoryDb, Usage},
    model::{Candidate, ItemKind, QueryResponse, Source},
    native::native_candidates,
    parser::parse_context,
    providers::{self, ProviderContext},
    ranking::{rank, rank_for_list},
    safety::sanitize_remote,
};
use anyhow::Result;
use std::{
    collections::HashMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Engine {
    pub cache: Cache,
    pub config: Config,
}
impl Engine {
    pub fn new(cache: Cache, config: Config) -> Self {
        Self { cache, config }
    }
    pub fn query(
        &self,
        buffer: &str,
        cursor: usize,
        cwd: &Path,
        offline: bool,
    ) -> Result<QueryResponse> {
        self.query_inner(buffer, cursor, cwd, offline, false)
    }
    pub fn list(
        &self,
        buffer: &str,
        cursor: usize,
        cwd: &Path,
        offline: bool,
    ) -> Result<QueryResponse> {
        self.query_inner(buffer, cursor, cwd, offline, true)
    }
    fn query_inner(
        &self,
        buffer: &str,
        cursor: usize,
        cwd: &Path,
        offline: bool,
        list: bool,
    ) -> Result<QueryResponse> {
        let context = parse_context(buffer, cursor);
        let mut candidates = providers::candidates(&ProviderContext {
            query: &context,
            cwd,
            cache: &self.cache,
            forge_enabled: self.config.providers.forge.enabled,
            docker_enabled: self.config.providers.docker.enabled,
            offline,
        });
        let has_contextual_values = !candidates.is_empty() && !context.current.starts_with('-');
        if !has_contextual_values && let Some(command) = context.command.as_deref() {
            let discoverer =
                Discoverer::new(&self.cache, self.config.completion.online_docs && !offline);
            if let Ok(root) = discoverer.discover(command, &[], false) {
                let mut native_args = context.args.clone();
                native_args.push(context.current.clone());
                candidates.extend(native_candidates(&root, command, &native_args, cwd));
                let mut schema = root;
                let mut path = Vec::new();
                for argument in &context.args {
                    let declares_command_slot = schema
                        .usage
                        .as_deref()
                        .is_some_and(|usage| usage.to_ascii_lowercase().contains("command"))
                        || schema
                            .items
                            .iter()
                            .any(|item| item.kind == ItemKind::Subcommand);
                    let is_subcommand = schema.items.iter().any(|item| {
                        item.kind == ItemKind::Subcommand && item.names.contains(argument)
                    }) || (declares_command_slot && !argument.starts_with('-'));
                    if is_subcommand {
                        path.push(argument.clone());
                        if let Ok(child) = discoverer.discover(command, &path, false) {
                            schema = child;
                        }
                    }
                }
                for item in schema.items {
                    if item.kind == ItemKind::Positional {
                        continue;
                    }
                    for name in item.names {
                        if !context.tokens.iter().any(|t| t == &name) {
                            let mut c =
                                Candidate::new(name, item.description.clone(), Source::LocalHelp);
                            c.confidence = schema.confidence;
                            if item.kind == ItemKind::Subcommand {
                                // Subcommands are what people are almost always
                                // reaching for at a bare "cmd " prompt; without
                                // this, an untyped TAB tie-breaks alphabetically
                                // and a wall of "--flags" (which sort before
                                // lowercase names) crowds them out entirely.
                                c.confidence += 0.04 + common_subcommand_bias(command, &c.value);
                            }
                            candidates.push(c)
                        }
                    }
                    for value in item.values {
                        candidates.push(Candidate::new(
                            value,
                            item.description.clone(),
                            Source::LocalHelp,
                        ));
                    }
                }
            }
        }
        let history = HistoryDb::load().unwrap_or_default();
        if candidates.is_empty() && self.config.completion.history && history.enabled {
            if let Some(cmd) = context.command.as_deref() {
                for (value, _) in history.matching(cmd, &context.current) {
                    candidates.push(Candidate::new(
                        value,
                        "learned from local history",
                        Source::History,
                    ));
                }
            }
        }
        let mut usage = HashMap::<String, Usage>::new();
        if let Some(command) = context.command.as_deref() {
            for (pattern, recorded) in &history.patterns {
                let Ok(words) = shell_words::split(pattern) else {
                    continue;
                };
                let next_index = 1 + context.args.len();
                if words.first().map(String::as_str) != Some(command)
                    || words.get(1..next_index) != Some(context.args.as_slice())
                    || next_index >= words.len()
                {
                    continue;
                }
                add_usage(&mut usage, words[next_index].clone(), recorded);
                add_usage(&mut usage, words[next_index..].join(" "), recorded);
                if command == "cd" && !words[next_index].ends_with('/') {
                    add_usage(&mut usage, format!("{}/", words[next_index]), recorded);
                }
            }
        }
        candidates.retain_mut(|candidate| {
            let Some(value) = sanitize_remote(&candidate.value) else {
                return false;
            };
            let Some(display) = sanitize_remote(&candidate.display) else {
                return false;
            };
            candidate.value = value;
            candidate.display = display;
            candidate.description = sanitize_remote(&candidate.description).unwrap_or_default();
            true
        });
        let mut candidates = if list {
            rank_for_list(
                candidates,
                &context.current,
                &usage,
                self.config.completion.fuzzy,
            )
        } else {
            rank(
                candidates,
                &context.current,
                &usage,
                self.config.completion.fuzzy,
            )
        };
        // The zsh frontend only auto-ghosts an empty (untyped) token when we
        // hand it back exactly one candidate -- otherwise it waits for the
        // user to type a prefix or press TAB, to avoid inventing tokens the
        // user never asked for. A habitual next argument (e.g. always "clone"
        // after "git") is exactly the kind of high-confidence case that rule
        // was meant to allow through, so collapse to that single candidate
        // here when usage history backs it decisively. Filesystem listings
        // (cd, etc.) are excluded: there the alternatives are always equally
        // real options to browse, not guesses to suppress.
        if !list
            && context.current.is_empty()
            && candidates.len() > 1
            && candidates[0].source != Source::Filesystem
        {
            let habitual = usage
                .get(&candidates[0].value)
                .is_some_and(|u| u.count >= 3)
                && candidates[0].score - candidates[1].score >= 15.0;
            if habitual {
                candidates.truncate(1);
            }
        }
        let max_candidates = self.config.ui.max_candidates.clamp(1, 50);
        candidates.truncate(
            if list
                && candidates
                    .first()
                    .is_some_and(|candidate| candidate.source == Source::Filesystem)
            {
                50
            } else {
                max_candidates
            },
        );
        Ok(QueryResponse {
            request_id: now_micros(),
            prefix_len: context.raw_current_len,
            candidates,
            cache_only: offline,
        })
    }
}
// A tiny nudge toward the subcommand most people mean, before any personal
// usage history exists to make that call itself (see the "habitual" collapse
// above, which takes over once real history backs a stronger choice). Ordered
// by how often each one actually gets reached for.
const COMMON_SUBCOMMANDS: &[(&str, &[&str])] = &[
    (
        "git",
        &[
            "clone", "pull", "push", "status", "commit", "checkout", "add", "branch", "log",
            "diff", "merge", "fetch", "init", "rebase", "stash", "remote",
        ],
    ),
    (
        "brew",
        &[
            "install", "uninstall", "update", "upgrade", "list", "search", "info", "cleanup",
            "services",
        ],
    ),
    (
        "npm",
        &[
            "install", "run", "start", "test", "build", "init", "update", "uninstall", "publish",
        ],
    ),
    (
        "docker",
        &[
            "run", "build", "ps", "exec", "stop", "start", "logs", "compose", "pull", "images",
        ],
    ),
    (
        "cargo",
        &[
            "build", "run", "test", "check", "add", "new", "update", "publish", "fmt", "clippy",
        ],
    ),
    (
        "kubectl",
        &["get", "apply", "delete", "describe", "logs", "exec", "create", "rollout"],
    ),
    ("go", &["run", "build", "test", "get", "mod", "install", "fmt", "vet"]),
    ("yarn", &["install", "add", "remove", "run", "start", "build"]),
    ("pnpm", &["install", "add", "remove", "run", "build"]),
];

fn common_subcommand_bias(command: &str, name: &str) -> f64 {
    COMMON_SUBCOMMANDS
        .iter()
        .find(|(cmd, _)| *cmd == command)
        .and_then(|(_, subs)| subs.iter().position(|sub| *sub == name))
        .map(|index| 0.05 - (index as f64 * 0.001))
        .unwrap_or(0.0)
}
fn add_usage(usage: &mut HashMap<String, Usage>, value: String, recorded: &Usage) {
    let entry = usage.entry(value).or_insert(Usage {
        count: 0,
        last_used: 0,
    });
    entry.count = entry.count.saturating_add(recorded.count);
    entry.last_used = entry.last_used.max(recorded.last_used);
}
fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
