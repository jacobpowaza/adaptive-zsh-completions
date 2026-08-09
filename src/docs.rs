use anyhow::{Context, Result, bail};
use regex::Regex;
use std::{collections::HashMap, time::Duration};

use crate::{
    cache::Cache,
    help_parser::{parse_help, strip_control},
    model::CommandSchema,
};

pub trait DocumentationResolver {
    fn resolve(&self, command: &str, path: &[String]) -> Result<Option<CommandSchema>>;
}

pub struct TrustedDocs<'a> {
    pub cache: &'a Cache,
}

impl TrustedDocs<'_> {
    pub fn url_for(command: &str, path: &[String]) -> Option<String> {
        let sub = path.first().map(String::as_str).unwrap_or("");
        match command {
            "git" => Some(format!(
                "https://git-scm.com/docs/git-{}",
                if sub.is_empty() { "git" } else { sub }
            )),
            "gh" => Some("https://cli.github.com/manual/gh".into()),
            "docker" => Some("https://docs.docker.com/reference/cli/docker/".into()),
            "kubectl" => Some("https://kubernetes.io/docs/reference/kubectl/generated/".into()),
            "npm" => Some("https://docs.npmjs.com/cli/using-npm/config".into()),
            "claude" => Some("https://docs.anthropic.com/en/docs/claude-code/cli-reference".into()),
            _ => None,
        }
    }
}

impl DocumentationResolver for TrustedDocs<'_> {
    fn resolve(&self, command: &str, path: &[String]) -> Result<Option<CommandSchema>> {
        let Some(url) = Self::url_for(command, path) else {
            return Ok(None);
        };
        let key = format!("{command}:{}", path.join("/"));
        if let Some(schema) = self.cache.get("docs", &key)? {
            return Ok(Some(schema));
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(4))
            .user_agent(format!("adaptive/{}", crate::VERSION))
            .build()?;
        let response = client
            .get(&url)
            .send()
            .with_context(|| format!("official documentation unavailable: {url}"))?;
        if !response.status().is_success() {
            bail!("official documentation returned {}", response.status())
        }
        if response
            .content_length()
            .is_some_and(|n| n > 2 * 1024 * 1024)
        {
            bail!("official documentation response too large")
        }
        let mut html = String::new();
        use std::io::Read;
        response.take(2 * 1024 * 1024).read_to_string(&mut html)?;
        let text = html_to_text(&html);
        let mut schema = parse_help(command, path, &text);
        schema.confidence = (schema.confidence * 0.95).min(0.91);
        self.cache
            .put("docs", &key, &schema, Some(Duration::from_secs(7 * 86400)))?;
        Ok(Some(schema))
    }
}

fn html_to_text(html: &str) -> String {
    let scripts = Regex::new(r"(?is)<script[^>]*>.*?</script>|<style[^>]*>.*?</style>")
        .unwrap()
        .replace_all(html, " ");
    let tags = Regex::new(r"(?s)<[^>]+>")
        .unwrap()
        .replace_all(&scripts, "\n");
    let entities: HashMap<&str, &str> = [
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&amp;", "&"),
        ("&quot;", "\""),
    ]
    .into();
    let mut text = tags.into_owned();
    for (from, to) in entities {
        text = text.replace(from, to);
    }
    strip_control(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_maps_trusted_commands() {
        assert!(
            TrustedDocs::url_for("git", &[])
                .unwrap()
                .starts_with("https://git-scm.com/")
        );
        assert!(TrustedDocs::url_for("mystery", &[]).is_none());
    }
}
