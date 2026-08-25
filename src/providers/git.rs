use super::{Provider, ProviderContext};
use crate::{
    model::{Candidate, Source},
    safety::run_successful_informational,
};
use anyhow::Result;
use std::{fs, path::Path, time::Duration};

const GIT_TIMEOUT: Duration = Duration::from_millis(500);

pub struct GitProvider;
impl Provider for GitProvider {
    fn name(&self) -> &'static str {
        "git"
    }
    fn matches(&self, c: &ProviderContext<'_>) -> bool {
        c.query.command.as_deref() == Some("git")
            && matches!(
                c.query.args.first().map(String::as_str),
                Some(
                    "checkout"
                        | "switch"
                        | "push"
                        | "remote"
                        | "branch"
                        | "worktree"
                        | "cherry-pick"
                )
            )
    }
    fn complete(&self, c: &ProviderContext<'_>) -> Result<Vec<Candidate>> {
        let args = &c.query.args;
        let sub = args.first().map(String::as_str).unwrap_or("");
        match sub {
            // A ref slot: every branch and remote-tracking name is valid, and
            // learned habits rank whichever ones this user actually visits.
            "checkout" | "switch" => refs(c, &["refs/heads", "refs/remotes"], "Git branch"),
            "push" => {
                if c.query.args.len() <= 1 {
                    remotes(c, "Git remote")
                } else {
                    refs(c, &["refs/heads"], "Git branch")
                }
            }
            "remote" => remotes(c, "Git remote"),
            // Deleting must stay valid: only local branches, never the one
            // that is checked out right now.
            "branch"
                if args
                    .iter()
                    .skip(1)
                    .any(|a| matches!(a.as_str(), "-d" | "-D" | "--delete")) =>
            {
                deletable_branches(c)
            }
            "branch" => refs(c, &["refs/heads"], "Local branch"),
            "cherry-pick"
                if !args
                    .iter()
                    .skip(1)
                    .any(|a| matches!(a.as_str(), "--continue" | "--abort" | "--quit")) =>
            {
                recent_commits(c)
            }
            "worktree" => worktrees(c),
            _ => Ok(Vec::new()),
        }
    }
}

/// Number of non-flag arguments after a sub-subcommand keyword (e.g. after
/// `add` within `git worktree add ...`). Flag values are miscounted as
/// positionals by this cheap heuristic; the slots it guards are tolerant.
fn positionals_after(args: &[String], keyword: &str) -> usize {
    let mut count = 0;
    let mut after = false;
    for arg in args.iter().skip(1) {
        if !after {
            after = arg == keyword;
            continue;
        }
        if !arg.starts_with('-') {
            count += 1;
        }
    }
    count
}

fn remotes(c: &ProviderContext<'_>, desc: &str) -> Result<Vec<Candidate>> {
    let text = run_successful_informational("git", ["remote"], Some(c.cwd), GIT_TIMEOUT)?;
    Ok(text
        .lines()
        .map(|s| Candidate::new(s.trim(), desc, Source::Dynamic))
        .collect())
}

fn refs(c: &ProviderContext<'_>, patterns: &[&str], desc: &str) -> Result<Vec<Candidate>> {
    let text = run_successful_informational(
        "git",
        ["for-each-ref", "--format=%(refname:short)"]
            .into_iter()
            .chain(patterns.iter().copied()),
        Some(c.cwd),
        GIT_TIMEOUT,
    )?;
    Ok(text
        .lines()
        .filter(|s| !s.contains(" -> "))
        .map(|s| Candidate::new(s.trim(), desc, Source::Dynamic))
        .collect())
}

/// Local branches that may legally be deleted: the checked-out branch is
/// excluded because refusing it at execution time is not a suggestion.
fn deletable_branches(c: &ProviderContext<'_>) -> Result<Vec<Candidate>> {
    let checked_out = run_successful_informational(
        "git",
        ["symbolic-ref", "--quiet", "--short", "HEAD"],
        Some(c.cwd),
        GIT_TIMEOUT,
    )
    .ok()
    .map(|text| text.trim().to_owned());
    Ok(refs(c, &["refs/heads"], "Deletable branch")?
        .into_iter()
        .filter(|candidate| Some(candidate.value.trim()) != checked_out.as_deref())
        .collect())
}

/// Recent commits as short hashes; the subject rides along in the display so
/// picking the right one stays a glance instead of a lookup.
fn recent_commits(c: &ProviderContext<'_>) -> Result<Vec<Candidate>> {
    let text = run_successful_informational(
        "git",
        ["log", "--max-count=25", "--format=%h%x09%s"],
        Some(c.cwd),
        GIT_TIMEOUT,
    )?;
    Ok(text
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(hash, subject)| {
            let mut candidate = Candidate::new(hash.trim(), "Recent commit", Source::Dynamic);
            candidate.display = format!("{} {}", hash.trim(), subject);
            candidate
        })
        .collect())
}

/// Worktree-aware completion:
/// - `worktree add <path>` offers sibling directories as sensible locations,
///   alongside the worktrees that already exist;
/// - `worktree add <path> <branch>` offers branches for the new worktree;
/// - every other slot (`list`, `remove`, `lock`, ...) offers the existing
///   worktrees themselves.
fn worktrees(c: &ProviderContext<'_>) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    let adding = c.query.args.get(1).is_some_and(|first| first == "add");
    if adding && positionals_after(&c.query.args, "add") == 0 {
        // First positional of `add`: where to put it. Sibling directories of
        // this repository keep the conventional `../name` layout reachable.
        out.extend(sibling_directories(c.cwd));
    } else if adding && positionals_after(&c.query.args, "add") == 1 {
        out.extend(refs(
            c,
            &["refs/heads", "refs/remotes"],
            "Branch for worktree",
        )?);
    }
    out.extend(
        existing_worktrees(c.cwd)
            .into_iter()
            .map(|path| Candidate::new(path.clone(), "Existing worktree", Source::Dynamic)),
    );
    Ok(out)
}

fn existing_worktrees(cwd: &Path) -> Vec<String> {
    let Ok(text) = run_successful_informational(
        "git",
        ["worktree", "list", "--porcelain"],
        Some(cwd),
        GIT_TIMEOUT,
    ) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(str::trim)
        .map(str::to_owned)
        .collect()
}

/// Directories beside the current repository, completed relative (`../name/`)
/// so the inserted text works from anywhere inside the repo.
fn sibling_directories(cwd: &Path) -> Vec<Candidate> {
    let Some(parent) = cwd.parent() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .take(50)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let mut candidate = Candidate::new(
                format!("../{}/", escape_shell_path(&name)),
                "directory",
                Source::Dynamic,
            );
            candidate.display = format!("../{name}/");
            Some(candidate)
        })
        .collect()
}

fn escape_shell_path(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '~') {
            escaped.push(ch);
        } else {
            escaped.push('\\');
            escaped.push(ch);
        }
    }
    escaped
}
