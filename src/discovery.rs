use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    cache::Cache,
    docs::{DocumentationResolver, TrustedDocs},
    help_parser::parse_help,
    model::{CommandSchema, ItemKind},
    safety::{run_informational, safe_help_args},
};

pub struct Discoverer<'a> {
    cache: &'a Cache,
    online_docs: bool,
}

impl<'a> Discoverer<'a> {
    pub fn new(cache: &'a Cache, online_docs: bool) -> Self {
        Self { cache, online_docs }
    }
    pub fn discover(&self, command: &str, path: &[String], refresh: bool) -> Result<CommandSchema> {
        let executable =
            find_executable(command).with_context(|| format!("command not found: {command}"))?;
        let fingerprint = fingerprint(&executable)?;
        let key = format!("{command}:{}", path.join("/"));
        if !refresh
            && let Some(schema) = self.cache.get::<CommandSchema>("schemas", &key)?
            && schema.executable_fingerprint == fingerprint
        {
            return Ok(schema);
        }
        let args = safe_help_args(path)?;
        let text = run_informational(
            executable.to_string_lossy().as_ref(),
            args,
            None,
            Duration::from_secs(2),
        )
        .or_else(|_| {
            if path.is_empty() {
                run_informational(
                    executable.to_string_lossy().as_ref(),
                    ["help"],
                    None,
                    Duration::from_secs(2),
                )
            } else {
                Err(anyhow::anyhow!("help unavailable"))
            }
        })?;
        let mut schema = parse_help(command, path, &text);
        schema.executable_fingerprint = fingerprint;
        schema.discovered_at = now();
        if self.online_docs && schema.items.len() < 3 {
            if let Ok(Some(official)) = (TrustedDocs { cache: self.cache }).resolve(command, path) {
                merge(&mut schema, official);
            }
        }
        self.cache.put("schemas", &key, &schema, None)?;
        Ok(schema)
    }
}

fn merge(local: &mut CommandSchema, official: CommandSchema) {
    for item in official.items {
        if !local
            .items
            .iter()
            .any(|i| i.kind == item.kind && i.names.iter().any(|n| item.names.contains(n)))
        {
            local.items.push(item)
        }
    }
    local.confidence = local.confidence.max(official.confidence);
}

pub fn child_path(schema: &CommandSchema, args: &[String]) -> Vec<String> {
    let mut path = schema.path.clone();
    for arg in args {
        if schema
            .items
            .iter()
            .any(|i| i.kind == ItemKind::Subcommand && i.names.contains(arg))
        {
            path.push(arg.clone());
            break;
        }
    }
    path
}

pub fn find_executable(command: &str) -> Result<PathBuf> {
    if command.contains('/') {
        let p = PathBuf::from(command);
        if p.is_file() {
            return Ok(p);
        } else {
            bail!("executable not found")
        }
    }
    if command.is_empty() || command.contains('\0') {
        bail!("invalid command")
    }
    for dir in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let p = dir.join(command);
        if p.is_file() {
            return Ok(p);
        }
    }
    bail!("executable not found")
}
fn fingerprint(path: &Path) -> Result<String> {
    let m = fs::metadata(path)?;
    let modified = m
        .modified()
        .unwrap_or(UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(format!(
        "{:x}",
        Sha256::digest(format!("{}:{}:{}", path.display(), m.len(), modified).as_bytes())
    ))
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
    #[test]
    fn command_lookup_finds_shell() {
        assert!(find_executable("sh").is_ok())
    }
}
