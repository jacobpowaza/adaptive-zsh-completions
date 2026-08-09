use super::{Provider, ProviderContext};
use crate::model::{Candidate, Source};
use anyhow::Result;
use std::{collections::BTreeSet, fs};
pub struct SshProvider;
impl Provider for SshProvider {
    fn name(&self) -> &'static str {
        "ssh"
    }
    fn matches(&self, c: &ProviderContext<'_>) -> bool {
        c.query.command.as_deref() == Some("ssh")
    }
    fn complete(&self, _: &ProviderContext<'_>) -> Result<Vec<Candidate>> {
        let mut hosts = BTreeSet::new();
        if let Some(home) = dirs::home_dir() {
            let config = home.join(".ssh/config");
            if let Ok(text) = fs::read_to_string(config) {
                for line in text.lines() {
                    let mut p = line.split_whitespace();
                    if p.next().is_some_and(|v| v.eq_ignore_ascii_case("host")) {
                        for h in p.filter(|h| !h.contains(['*', '?', '!'])) {
                            hosts.insert(h.to_owned());
                        }
                    }
                }
            }
            let known = home.join(".ssh/known_hosts");
            if let Ok(text) = fs::read_to_string(known) {
                for line in text.lines().take(10000) {
                    if line.starts_with('|') {
                        continue;
                    }
                    if let Some(field) = line.split_whitespace().next() {
                        for host in field.split(',') {
                            let host = host
                                .trim_matches(&['[', ']'][..])
                                .split(':')
                                .next()
                                .unwrap_or(host);
                            if !host.is_empty() {
                                hosts.insert(host.into());
                            }
                        }
                    }
                }
            }
        }
        Ok(hosts
            .into_iter()
            .map(|h| Candidate::new(h, "SSH host", Source::Dynamic))
            .collect())
    }
}
