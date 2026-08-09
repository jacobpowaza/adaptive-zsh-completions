use std::{env, io::Read, time::Duration};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, de::DeserializeOwned};

use super::{Provider, ProviderContext};
use crate::{
    model::{Candidate, Source},
    safety::{run_successful_informational, sanitize_remote},
};

const RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneStyle {
    Https,
    Ssh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForgeKind {
    Github,
    Gitlab,
    Gitea,
    Bitbucket,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeInput {
    pub style: CloneStyle,
    pub host: String,
    pub owner: String,
    pub repo_prefix: String,
}

pub fn parse_forge_input(value: &str) -> Option<ForgeInput> {
    let (style, host, rest) = if let Some(rest) = value.strip_prefix("https://") {
        let (host, path) = rest.split_once('/')?;
        (CloneStyle::Https, host, path)
    } else {
        let rest = value.strip_prefix("git@")?;
        let (host, path) = rest.split_once(':')?;
        (CloneStyle::Ssh, host, path)
    };
    forge_kind(host)?;
    let mut parts = rest.splitn(2, '/');
    let owner = parts.next()?.trim();
    if owner.is_empty()
        || !owner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return None;
    }
    let repo = parts.next()?.trim_end_matches(".git");
    if repo.contains('/')
        || !repo
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return None;
    }
    Some(ForgeInput {
        style,
        host: host.to_ascii_lowercase(),
        owner: owner.into(),
        repo_prefix: repo.into(),
    })
}

fn forge_kind(host: &str) -> Option<ForgeKind> {
    match host.to_ascii_lowercase().as_str() {
        "github.com" => Some(ForgeKind::Github),
        "gitlab.com" => Some(ForgeKind::Gitlab),
        "codeberg.org" | "gitea.com" => Some(ForgeKind::Gitea),
        "bitbucket.org" => Some(ForgeKind::Bitbucket),
        _ => None,
    }
}

pub struct ForgeProvider;

impl Provider for ForgeProvider {
    fn name(&self) -> &'static str {
        "forge.repositories"
    }

    fn matches(&self, context: &ProviderContext<'_>) -> bool {
        context.forge_enabled
            && context.query.command.as_deref() == Some("git")
            && context.query.args.first().is_some_and(|arg| arg == "clone")
    }

    fn complete(&self, context: &ProviderContext<'_>) -> Result<Vec<Candidate>> {
        if context.query.current.is_empty() {
            return Ok(vec![Candidate::new(
                "https://github.com/",
                "repository forge",
                Source::Dynamic,
            )]);
        }
        let Some(input) = parse_forge_input(&context.query.current) else {
            return Ok(Vec::new());
        };
        let key = format!("repos:{}:{}", input.host, input.owner.to_ascii_lowercase());
        let repositories: Vec<Repository> =
            if let Some(cached) = context.cache.get("forge", &key)? {
                cached
            } else {
                if context.offline {
                    return Ok(Vec::new());
                }
                let repositories = fetch(&input)?;
                context
                    .cache
                    .put("forge", &key, &repositories, Some(Duration::from_secs(300)))?;
                repositories
            };
        let prefix = input.repo_prefix.to_ascii_lowercase();
        Ok(repositories
            .into_iter()
            .filter(|repository| repository.name.to_ascii_lowercase().contains(&prefix))
            .filter_map(|repository| {
                let value = match input.style {
                    CloneStyle::Https => format!(
                        "https://{}/{}/{}.git",
                        input.host, input.owner, repository.name
                    ),
                    CloneStyle::Ssh => {
                        format!("git@{}:{}/{}.git", input.host, input.owner, repository.name)
                    }
                };
                let mut candidate = Candidate::new(
                    sanitize_remote(&value)?,
                    repository
                        .description
                        .unwrap_or_else(|| "forge repository".into()),
                    Source::Dynamic,
                );
                candidate.display = format!("{}/{}.git", input.owner, repository.name);
                Some(candidate)
            })
            .collect())
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
struct Repository {
    name: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct GithubRepository {
    name: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct GitlabRepository {
    path: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct GiteaRepository {
    name: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct BitbucketRepositories {
    values: Vec<BitbucketRepository>,
}

#[derive(Deserialize)]
struct BitbucketRepository {
    slug: String,
    description: Option<String>,
}

fn fetch(input: &ForgeInput) -> Result<Vec<Repository>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .user_agent(format!("adaptive/{}", crate::VERSION))
        .build()?;
    let kind = forge_kind(&input.host).ok_or_else(|| anyhow::anyhow!("unsupported forge host"))?;
    match kind {
        ForgeKind::Github => {
            let base =
                env::var("ADAPTIVE_GITHUB_API").unwrap_or_else(|_| "https://api.github.com".into());
            let url = format!(
                "{}/users/{}/repos?per_page=100&type=owner&sort=updated",
                base.trim_end_matches('/'),
                input.owner
            );
            let repositories: Vec<GithubRepository> =
                get_json(&client, &url, github_token().as_deref())?;
            Ok(repositories
                .into_iter()
                .map(|repo| Repository {
                    name: repo.name,
                    description: repo.description,
                })
                .collect())
        }
        ForgeKind::Gitlab => {
            let base = env::var("ADAPTIVE_GITLAB_API")
                .unwrap_or_else(|_| "https://gitlab.com/api/v4".into());
            let url = format!(
                "{}/users/{}/projects?per_page=100&order_by=last_activity_at",
                base.trim_end_matches('/'),
                input.owner
            );
            let repositories: Vec<GitlabRepository> = get_json(&client, &url, None)?;
            Ok(repositories
                .into_iter()
                .map(|repo| Repository {
                    name: repo.path,
                    description: repo.description,
                })
                .collect())
        }
        ForgeKind::Gitea => {
            let base = env::var("ADAPTIVE_GITEA_API")
                .unwrap_or_else(|_| format!("https://{}/api/v1", input.host));
            let url = format!(
                "{}/users/{}/repos?limit=100",
                base.trim_end_matches('/'),
                input.owner
            );
            let repositories: Vec<GiteaRepository> = get_json(&client, &url, None)?;
            Ok(repositories
                .into_iter()
                .map(|repo| Repository {
                    name: repo.name,
                    description: repo.description,
                })
                .collect())
        }
        ForgeKind::Bitbucket => {
            let base = env::var("ADAPTIVE_BITBUCKET_API")
                .unwrap_or_else(|_| "https://api.bitbucket.org/2.0".into());
            let url = format!(
                "{}/repositories/{}?pagelen=100",
                base.trim_end_matches('/'),
                input.owner
            );
            let repositories: BitbucketRepositories = get_json(&client, &url, None)?;
            Ok(repositories
                .values
                .into_iter()
                .map(|repo| Repository {
                    name: repo.slug,
                    description: repo.description,
                })
                .collect())
        }
    }
}

fn get_json<T: DeserializeOwned>(
    client: &reqwest::blocking::Client,
    url: &str,
    bearer_token: Option<&str>,
) -> Result<T> {
    let mut request = client.get(url).header("Accept", "application/json");
    if let Some(token) = bearer_token {
        request = request.bearer_auth(token);
    }
    let response = request.send().context("forge provider request failed")?;
    if !response.status().is_success() {
        bail!("forge provider returned {}", response.status())
    }
    if response
        .content_length()
        .is_some_and(|length| length > RESPONSE_LIMIT as u64)
    {
        bail!("forge response too large")
    }
    let mut bytes = Vec::new();
    response
        .take(RESPONSE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > RESPONSE_LIMIT {
        bail!("forge response too large")
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn github_token() -> Option<String> {
    env::var("GH_TOKEN")
        .or_else(|_| env::var("GITHUB_TOKEN"))
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let token = run_successful_informational(
                "gh",
                ["auth", "token"],
                None,
                Duration::from_millis(500),
            )
            .ok()?
            .trim()
            .to_owned();
            (!token.is_empty()).then_some(token)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_and_ssh_across_forges() {
        let github = parse_forge_input("https://github.com/jacob/r").unwrap();
        assert_eq!(github.repo_prefix, "r");
        assert_eq!(github.host, "github.com");
        let gitlab = parse_forge_input("git@gitlab.com:jacob/repo.git").unwrap();
        assert_eq!(gitlab.style, CloneStyle::Ssh);
        assert_eq!(gitlab.host, "gitlab.com");
        assert!(parse_forge_input("https://codeberg.org/knut/example").is_some());
        assert!(parse_forge_input("https://bitbucket.org/team/repo").is_some());
    }

    #[test]
    fn rejects_unknown_hosts_and_injection() {
        assert!(parse_forge_input("https://evil.example/a/repo").is_none());
        assert!(parse_forge_input("https://github.com/a/repo\u{1b}").is_none());
    }
}
