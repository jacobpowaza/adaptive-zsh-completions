use super::{Provider, ProviderContext};
use crate::{
    model::{Candidate, Source},
    safety::sanitize_remote,
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    env,
    process::{Command, Stdio},
    time::Duration,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubStyle {
    Https,
    Ssh,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubInput {
    pub style: GithubStyle,
    pub owner: String,
    pub repo_prefix: String,
}
pub fn parse_github_input(value: &str) -> Option<GithubInput> {
    let (style, rest) = if let Some(v) = value.strip_prefix("https://github.com/") {
        (GithubStyle::Https, v)
    } else {
        (GithubStyle::Ssh, value.strip_prefix("git@github.com:")?)
    };
    let mut p = rest.splitn(2, '/');
    let owner = p.next()?.trim();
    if owner.is_empty() || !owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let repo = p.next()?.trim_end_matches(".git");
    if repo.contains('/')
        || !repo
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return None;
    }
    Some(GithubInput {
        style,
        owner: owner.into(),
        repo_prefix: repo.into(),
    })
}
pub struct GithubProvider;
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
struct Repo {
    name: String,
    html_url: String,
    clone_url: String,
    ssh_url: String,
    description: Option<String>,
    fork: bool,
}
impl Provider for GithubProvider {
    fn name(&self) -> &'static str {
        "github.repositories"
    }
    fn matches(&self, c: &ProviderContext<'_>) -> bool {
        c.github_enabled
            && c.query.command.as_deref() == Some("git")
            && c.query.args.first().is_some_and(|s| s == "clone")
    }
    fn complete(&self, c: &ProviderContext<'_>) -> Result<Vec<Candidate>> {
        if c.query.current.is_empty() {
            return Ok(vec![Candidate::new(
                "https://github.com/",
                "GitHub repository",
                Source::Dynamic,
            )]);
        }
        let Some(input) = parse_github_input(&c.query.current) else {
            return Ok(Vec::new());
        };
        let key = format!("repos:{}", input.owner.to_ascii_lowercase());
        let repos: Vec<Repo> = if let Some(cached) = c.cache.get("github", &key)? {
            cached
        } else {
            if c.offline {
                return Ok(Vec::new());
            }
            let repos = fetch(&input.owner)?;
            c.cache
                .put("github", &key, &repos, Some(Duration::from_secs(300)))?;
            repos
        };
        Ok(repos
            .into_iter()
            .filter(|r| {
                r.name
                    .to_ascii_lowercase()
                    .contains(&input.repo_prefix.to_ascii_lowercase())
            })
            .filter_map(|r| {
                let value = match input.style {
                    GithubStyle::Https => {
                        format!("https://github.com/{}/{}.git", input.owner, r.name)
                    }
                    GithubStyle::Ssh => format!("git@github.com:{}/{}.git", input.owner, r.name),
                };
                let safe = sanitize_remote(&value)?;
                let mut c = Candidate::new(
                    safe,
                    r.description.unwrap_or_else(|| "GitHub repository".into()),
                    Source::Dynamic,
                );
                c.display = format!("{}/{}.git", input.owner, r.name);
                Some(c)
            })
            .collect())
    }
}
fn fetch(owner: &str) -> Result<Vec<Repo>> {
    let base = env::var("ADAPTIVE_GITHUB_API").unwrap_or_else(|_| "https://api.github.com".into());
    let url = format!(
        "{}/users/{}/repos?per_page=100&type=owner&sort=updated",
        base.trim_end_matches('/'),
        owner
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .user_agent(format!("adaptive/{}", crate::VERSION))
        .build()?;
    let mut request = client
        .get(url)
        .header("Accept", "application/vnd.github+json");
    if let Some(token) = github_token() {
        request = request.bearer_auth(token)
    }
    let response = request.send().context("GitHub provider request failed")?;
    if !response.status().is_success() {
        bail!("GitHub provider returned {}", response.status())
    }
    if response
        .content_length()
        .is_some_and(|n| n > 2 * 1024 * 1024)
    {
        bail!("GitHub response too large")
    }
    Ok(response.json()?)
}
fn github_token() -> Option<String> {
    env::var("GH_TOKEN")
        .or_else(|_| env::var("GITHUB_TOKEN"))
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            let out = Command::new("gh")
                .args(["auth", "token"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let t = String::from_utf8(out.stdout).ok()?.trim().to_owned();
            (!t.is_empty()).then_some(t)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_https_and_ssh() {
        assert_eq!(
            parse_github_input("https://github.com/jacob/r")
                .unwrap()
                .repo_prefix,
            "r"
        );
        assert_eq!(
            parse_github_input("git@github.com:jacob/repo.git")
                .unwrap()
                .style,
            GithubStyle::Ssh
        )
    }
    #[test]
    fn rejects_injection() {
        assert!(parse_github_input("https://github.com/a/repo\u{1b}").is_none())
    }
}
