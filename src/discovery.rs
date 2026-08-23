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
            && let Some(schema) = self.cache.get::<CommandSchema>(&schema_namespace(), &key)?
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
        .unwrap_or_default();
        let mut schema = parse_help(command, path, &text);
        // Symfony Console applications (poetry, composer, phpstan, ...) answer
        // bare `--help` with the help of their hidden `list` command instead
        // of a command table, so the probes below must still run even though
        // flags were parsed.
        let symfony_console = text
            .to_ascii_lowercase()
            .contains("display help for the given command");
        if command == "git"
            && path.is_empty()
            && let Ok(text) = run_informational(
                executable.to_string_lossy().as_ref(),
                ["help", "-a"],
                None,
                Duration::from_secs(2),
            )
        {
            merge(&mut schema, parse_git_main_commands(&text));
        }
        if schema.items.is_empty() {
            let mut short = path.to_vec();
            short.push("-h".into());
            if let Ok(text) = run_informational(
                executable.to_string_lossy().as_ref(),
                short,
                None,
                Duration::from_secs(2),
            ) {
                schema = parse_help(command, path, &text);
            }
        }
        if schema.items.is_empty() {
            let mut help = vec!["help".to_owned()];
            help.extend(path.iter().cloned());
            if let Ok(text) = run_informational(
                executable.to_string_lossy().as_ref(),
                help,
                None,
                Duration::from_secs(2),
            ) {
                schema = parse_help(command, path, &text);
            }
        }
        // Symfony Console applications (poetry, composer, phpstan, ...) answer
        // bare `--help` with the help of their hidden `list` command instead
        // of a command table; asking them for `list` directly prints one.
        // Accepted only when it actually yields a listing, because a bare
        // subcommand probe is the loosest interface in this chain.
        if schema.items.is_empty()
            || (symfony_console
                && !schema
                    .items
                    .iter()
                    .any(|item| item.kind == ItemKind::Subcommand))
        {
            let mut listing = path.to_vec();
            listing.push("list".into());
            if let Ok(text) = run_informational(
                executable.to_string_lossy().as_ref(),
                listing,
                None,
                Duration::from_secs(2),
            ) {
                let parsed = parse_help(command, path, &text);
                if parsed
                    .items
                    .iter()
                    .filter(|item| item.kind == ItemKind::Subcommand)
                    .count()
                    >= 2
                {
                    schema = parsed;
                }
            }
        }
        if schema.items.is_empty()
            && command
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            let topic = std::iter::once(command)
                .chain(path.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join("-");
            if let Ok(text) = run_informational(
                "man",
                ["-P", "cat", topic.as_str()],
                None,
                Duration::from_secs(2),
            ) {
                schema = parse_help(command, path, &text);
                schema.confidence = schema.confidence.min(0.68);
            }
        }
        if schema.items.is_empty() {
            bail!("no completion metadata discovered for {command}");
        }
        schema.executable_fingerprint = fingerprint;
        schema.discovered_at = now();
        if self.online_docs && schema.items.len() < 3 {
            if let Ok(Some(official)) = (TrustedDocs { cache: self.cache }).resolve(command, path) {
                merge(&mut schema, official);
            }
        }
        self.cache.put(&schema_namespace(), &key, &schema, None)?;
        Ok(schema)
    }
}

pub fn schema_namespace() -> String {
    format!("schemas-{}-3", crate::VERSION)
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

fn parse_git_main_commands(text: &str) -> CommandSchema {
    let mut section = String::from("Commands:\n");
    let mut in_main = false;
    for line in text.lines() {
        if line.trim() == "Main Porcelain Commands" {
            in_main = true;
            continue;
        }
        if !in_main {
            continue;
        }
        if !line.is_empty() && line.trim_start().len() == line.len() {
            break;
        }
        section.push_str(line);
        section.push('\n');
    }
    parse_help("git", &[], &section)
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
