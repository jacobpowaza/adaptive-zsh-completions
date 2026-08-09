use super::{Provider, ProviderContext};
use crate::model::{Candidate, Source};
use anyhow::Result;
use std::{
    collections::hash_map::DefaultHasher,
    env, fs,
    hash::{Hash, Hasher},
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Indexes executables on PATH so bare command names complete without history.
pub struct PathCommandProvider;

impl Provider for PathCommandProvider {
    fn name(&self) -> &'static str {
        "path-commands"
    }
    fn matches(&self, c: &ProviderContext<'_>) -> bool {
        c.query.tokens.is_empty() && !c.query.current.is_empty() && !c.query.current.contains('/')
    }
    fn complete(&self, c: &ProviderContext<'_>) -> Result<Vec<Candidate>> {
        let names = indexed_path_commands(c)?;
        let prefix = &c.query.current;
        Ok(names
            .iter()
            .filter(|name| name.starts_with(prefix.as_str()))
            .take(200)
            .map(|name| {
                let mut cand = Candidate::new(name.clone(), "command on PATH", Source::Command);
                cand.confidence = common_bias(name);
                cand
            })
            .collect())
    }
}

fn common_bias(name: &str) -> f64 {
    // A tiny nudge for everyday tools so they surface first on day one, before
    // any personal usage history exists. Ordered roughly by how often each
    // tool actually gets reached for -- earlier entries win ties against
    // later ones (e.g. "git" outranks "gh"). Not an exhaustive command
    // database.
    const COMMON: &[&str] = &[
        "git", "cd", "ls", "npm", "docker", "cargo", "python3", "ssh", "vim", "code",
        "curl", "brew", "node", "grep", "make", "go", "kubectl", "cat", "gh", "pnpm",
        "yarn", "rustc", "wget", "claude", "ffmpeg", "man", "less", "find", "sed", "awk", "tar",
    ];
    let base = Source::Command.base_confidence();
    match COMMON.iter().position(|&candidate| candidate == name) {
        Some(index) => base + 0.08 - (index as f64 * 0.001),
        None => base,
    }
}

fn indexed_path_commands(c: &ProviderContext<'_>) -> Result<Vec<String>> {
    let path_var = env::var_os("PATH").unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    path_var.hash(&mut hasher);
    let key = format!("path-index:{:x}", hasher.finish());
    if let Some(cached) = c.cache.get::<Vec<String>>("commands", &key)? {
        return Ok(cached);
    }
    let mut names = std::collections::BTreeSet::new();
    for dir in env::split_paths(&path_var) {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                continue;
            }
            if !is_executable(&entry) {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                names.insert(name.to_owned());
            }
        }
    }
    let names: Vec<String> = names.into_iter().collect();
    c.cache
        .put("commands", &key, &names, Some(Duration::from_secs(600)))?;
    Ok(names)
}

#[cfg(unix)]
fn is_executable(entry: &fs::DirEntry) -> bool {
    entry
        .metadata()
        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}
#[cfg(not(unix))]
fn is_executable(_entry: &fs::DirEntry) -> bool {
    true
}
