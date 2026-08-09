use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub completion: CompletionConfig,
    pub ui: UiConfig,
    pub providers: ProvidersConfig,
    pub privacy: PrivacyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompletionConfig {
    pub history: bool,
    pub online_docs: bool,
    pub fuzzy: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub ghost_text: bool,
    pub menu: bool,
    pub max_candidates: usize,
    pub enter_accepts_menu: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProvidersConfig {
    pub github: ProviderToggle,
    pub docker: ProviderToggle,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderToggle {
    pub enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PrivacyConfig {
    pub telemetry: bool,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            history: true,
            online_docs: false,
            fuzzy: true,
        }
    }
}
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            ghost_text: true,
            menu: true,
            max_candidates: 8,
            enter_accepts_menu: false,
        }
    }
}
impl Default for ProviderToggle {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        std::env::var_os("ADAPTIVE_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::config_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("adaptive/config.toml")
            })
    }
    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let config: Self =
            toml::from_str(&raw).with_context(|| format!("invalid config {}", path.display()))?;
        if config.privacy.telemetry {
            anyhow::bail!("telemetry is not supported; set privacy.telemetry = false");
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_partial_config_with_defaults() {
        let c: Config =
            toml::from_str("[ui]\nmax_candidates = 3\n[completion]\nfuzzy = false").unwrap();
        assert_eq!(c.ui.max_candidates, 3);
        assert!(!c.completion.fuzzy);
        assert!(c.completion.history);
    }
    #[test]
    fn telemetry_defaults_off() {
        assert!(!Config::default().privacy.telemetry);
    }
}
