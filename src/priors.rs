//! Global usage priors: how likely a token is at this position for *anybody*,
//! before this particular user's history has an opinion.
//!
//! Ranking needs three inputs to feel predictive rather than literal: what is
//! valid here (discovery), what this user does (history), and what people in
//! general do (this module). Without the third, a fresh install can only sort
//! alphabetically and a bare `brew <TAB>` answers with `--debug`; with it, the
//! same prompt answers `install`. Priors are deliberately weak signals -- they
//! decide ties and empty-prefix predictions, and personal history overrides
//! them as soon as it exists.

use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Priors {
    /// Bare command names, most commonly used first.
    pub commands: Vec<String>,
    /// Continuations keyed by full command path ("git", "docker compose").
    pub subcommands: HashMap<String, Vec<String>>,
    /// Overrides applied when the cwd is inside a Git work tree.
    pub in_git_repo: HashMap<String, Vec<String>>,
    /// Overrides applied when a package.json is in scope.
    pub in_node_project: HashMap<String, Vec<String>>,
}

/// Which of the context-specific tables apply to the current directory.
/// Resolved once per query so ranking never touches the filesystem per
/// candidate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scope {
    pub git_repo: bool,
    pub node_project: bool,
}

impl Scope {
    pub fn detect(cwd: &Path) -> Self {
        let mut scope = Self::default();
        for dir in cwd.ancestors().take(40) {
            if !scope.git_repo && dir.join(".git").exists() {
                scope.git_repo = true;
            }
            if !scope.node_project && dir.join("package.json").is_file() {
                scope.node_project = true;
            }
            if scope.git_repo && scope.node_project {
                break;
            }
        }
        scope
    }
}

const BUILTIN: &str = include_str!("../data/priors.toml");

pub fn priors() -> &'static Priors {
    static CACHE: OnceLock<Priors> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut priors: Priors = toml::from_str(BUILTIN).unwrap_or_default();
        if let Some(user) = user_priors() {
            priors.merge(user);
        }
        priors
    })
}

impl Priors {
    /// User entries replace the built-in list for a key outright, so a command
    /// only has to be named once to be reordered rather than appended to.
    fn merge(&mut self, user: Priors) {
        if !user.commands.is_empty() {
            self.commands = user.commands;
        }
        self.subcommands.extend(user.subcommands);
        self.in_git_repo.extend(user.in_git_repo);
        self.in_node_project.extend(user.in_node_project);
    }

    /// Ordered continuations for a command path, most likely first.
    pub fn next_tokens(&self, key: &str, scope: Scope) -> &[String] {
        if scope.git_repo
            && let Some(list) = self.in_git_repo.get(key)
        {
            return list;
        }
        if scope.node_project
            && let Some(list) = self.in_node_project.get(key)
        {
            return list;
        }
        self.subcommands.get(key).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Prior for a continuation of `key`, in 0.0..=1.0 (0.0 = not listed).
    pub fn token_prior(&self, key: &str, token: &str, scope: Scope) -> f64 {
        weight(self.next_tokens(key, scope), token)
    }

    /// Prior for a bare command name, in 0.0..=1.0 (0.0 = not listed).
    pub fn command_prior(&self, name: &str) -> f64 {
        weight(&self.commands, name)
    }
}

/// Rank-to-weight curve: first entry 1.0, last entry 0.3, so being on a list
/// at all counts for more than one's exact position on it. The lists are
/// authored by hand and their tails are guesses; the head is what we are
/// actually confident about.
fn weight(list: &[String], value: &str) -> f64 {
    let Some(index) = list.iter().position(|entry| entry == value) else {
        return 0.0;
    };
    if list.len() <= 1 {
        return 1.0;
    }
    1.0 - 0.7 * (index as f64 / (list.len() - 1) as f64)
}

/// The command-path key for a completion position: the command plus every
/// preceding subcommand ("gh pr" when completing the token after `gh pr `).
pub fn path_key(command: &str, path: &[String]) -> String {
    let mut key = String::from(command);
    for segment in path {
        key.push(' ');
        key.push_str(segment);
    }
    key
}

fn user_priors() -> Option<Priors> {
    let raw = std::fs::read_to_string(user_path()).ok()?;
    toml::from_str(&raw).ok()
}

pub fn user_path() -> PathBuf {
    std::env::var_os("ADAPTIVE_PRIORS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("adaptive/priors.toml")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_table_parses_and_orders_by_position() {
        let p = priors();
        let plain = Scope::default();
        assert!(p.token_prior("brew", "install", plain) > p.token_prior("brew", "cleanup", plain));
        assert!(p.token_prior("brew", "cleanup", plain) > 0.0);
        assert_eq!(p.token_prior("brew", "--debug", plain), 0.0);
        assert!(p.command_prior("git") > p.command_prior("gh"));
        assert_eq!(p.command_prior("definitely-not-a-real-command"), 0.0);
    }

    #[test]
    fn nested_paths_have_their_own_priors() {
        let p = priors();
        let plain = Scope::default();
        assert_eq!(
            p.next_tokens(&path_key("docker", &["compose".into()]), plain)
                .first()
                .map(String::as_str),
            Some("up")
        );
        assert_eq!(
            p.next_tokens(&path_key("gh", &["pr".into()]), plain)
                .first()
                .map(String::as_str),
            Some("create")
        );
    }

    #[test]
    fn directory_context_reorders_the_same_command() {
        let p = priors();
        assert_eq!(
            p.next_tokens("git", Scope::default())
                .first()
                .map(String::as_str),
            Some("clone")
        );
        assert_eq!(
            p.next_tokens(
                "git",
                Scope {
                    git_repo: true,
                    ..Scope::default()
                }
            )
            .first()
            .map(String::as_str),
            Some("status")
        );
        assert_eq!(
            p.next_tokens(
                "npm",
                Scope {
                    node_project: true,
                    ..Scope::default()
                }
            )
            .first()
            .map(String::as_str),
            Some("run")
        );
    }
}
