use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HistoryDb {
    pub enabled: bool,
    pub patterns: HashMap<String, Usage>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub count: u64,
    pub last_used: u64,
}

impl HistoryDb {
    pub fn load() -> Result<Self> {
        let p = path();
        if !p.exists() {
            return Ok(Self {
                enabled: true,
                ..Default::default()
            });
        }
        Ok(serde_json::from_slice(&fs::read(p)?)?)
    }
    pub fn save(&self) -> Result<()> {
        let p = path();
        if let Some(d) = p.parent() {
            fs::create_dir_all(d)?;
        }
        let t = p.with_extension("tmp");
        fs::write(&t, serde_json::to_vec(self)?)?;
        fs::rename(t, p)?;
        Ok(())
    }
    pub fn record(&mut self, command: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let Some(pattern) = sanitize(command) else {
            return false;
        };
        let usage = self.patterns.entry(pattern).or_insert(Usage {
            count: 0,
            last_used: 0,
        });
        usage.count += 1;
        usage.last_used = now();
        true
    }
    pub fn clear(&mut self) {
        self.patterns.clear();
    }
    pub fn matching(&self, command: &str, prefix: &str) -> Vec<(String, Usage)> {
        self.patterns
            .iter()
            .filter_map(|(pattern, usage)| {
                let rest = pattern.strip_prefix(command)?.trim_start();
                if rest.starts_with(prefix) {
                    Some((rest.to_owned(), usage.clone()))
                } else {
                    None
                }
            })
            .collect()
    }
}

pub fn sanitize(input: &str) -> Option<String> {
    let value = input.trim();
    if value.is_empty()
        || value.len() > 512
        || value.contains('\n')
        || value.contains('\r')
        || value.contains('\0')
    {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    let secret_words = [
        "authorization:",
        "bearer ",
        "password=",
        "passwd=",
        "api_key",
        "apikey",
        "access_token",
        "secret=",
        "private key",
        "--password",
        "--token",
    ];
    if secret_words.iter().any(|s| lower.contains(s)) {
        return None;
    }
    let jwt = Regex::new(r"eyJ[A-Za-z0-9_-]{15,}\.[A-Za-z0-9_-]{15,}\.[A-Za-z0-9_-]{10,}").unwrap();
    let random = Regex::new(r"(?i)(?:[a-z0-9+/=_-]{48,})").unwrap();
    let assignment = Regex::new(r"(?:^|\s)[A-Za-z_][A-Za-z0-9_]{1,40}=[^\s]{12,}").unwrap();
    if jwt.is_match(value) || random.is_match(value) || assignment.is_match(value) {
        return None;
    }
    let words = shell_words::split(value).ok()?;
    if words.len() > 24 || words.iter().any(|w| w.len() > 160) {
        return None;
    }
    Some(words.join(" "))
}
fn path() -> PathBuf {
    std::env::var_os("ADAPTIVE_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("adaptive")
        })
        .join("history.json")
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
    fn accepts_useful_short_commands() {
        assert_eq!(
            sanitize("git push origin dev").as_deref(),
            Some("git push origin dev")
        );
    }
    #[test]
    fn rejects_secrets_and_pastes() {
        assert!(sanitize("curl -H 'Authorization: Bearer secretvalue' x").is_none());
        assert!(sanitize("TOKEN=abcdefghijklmnopqrstuvwxyz123456 command").is_none());
        assert!(sanitize("echo eyJaaaaaaaaaaaaaaaa.bbbbbbbbbbbbbbbbb.cccccccccc").is_none());
        assert!(sanitize("echo a\necho b").is_none());
    }
}
