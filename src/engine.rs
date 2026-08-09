use crate::{
    cache::Cache,
    config::Config,
    discovery::{Discoverer, child_path},
    history::HistoryDb,
    model::{Candidate, ItemKind, QueryResponse, Source},
    native::native_candidates,
    parser::parse_context,
    providers::{self, ProviderContext},
    ranking::rank,
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
        let context = parse_context(buffer, cursor);
        let mut candidates = providers::candidates(&ProviderContext {
            query: &context,
            cwd,
            cache: &self.cache,
            github_enabled: self.config.providers.github.enabled,
            docker_enabled: self.config.providers.docker.enabled,
            offline,
        });
        if let Some(command) = context.command.as_deref() {
            let discoverer =
                Discoverer::new(&self.cache, self.config.completion.online_docs && !offline);
            if let Ok(root) = discoverer.discover(command, &[], false) {
                let mut native_args = context.args.clone();
                native_args.push(context.current.clone());
                candidates.extend(native_candidates(&root, command, &native_args, cwd));
                let path = child_path(&root, &context.args);
                let schema = if path.is_empty() {
                    root
                } else {
                    discoverer.discover(command, &path, false).unwrap_or(root)
                };
                for item in schema.items {
                    if item.kind == ItemKind::Positional {
                        continue;
                    }
                    for name in item.names {
                        if !context.tokens.iter().any(|t| t == &name) {
                            let mut c =
                                Candidate::new(name, item.description.clone(), Source::LocalHelp);
                            c.confidence = schema.confidence;
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
        let usage: HashMap<_, _> = history
            .patterns
            .iter()
            .filter_map(|(p, u)| {
                let cmd = context.command.as_deref()?;
                Some((p.strip_prefix(cmd)?.trim_start().to_owned(), u.clone()))
            })
            .collect();
        let mut candidates = rank(
            candidates,
            &context.current,
            &usage,
            self.config.completion.fuzzy,
        );
        candidates.truncate(self.config.ui.max_candidates.clamp(1, 50));
        Ok(QueryResponse {
            request_id: now_micros(),
            prefix_len: context.current.chars().count(),
            candidates,
            cache_only: offline,
        })
    }
}
fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
