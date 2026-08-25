use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub completion: CompletionConfig,
    pub ui: UiConfig,
    pub providers: ProvidersConfig,
    /// Optional `carapace-bin` integration: an external completion engine
    /// consulted when present, never required (see `crate::carapace`).
    pub carapace: CarapaceConfig,
    pub privacy: PrivacyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompletionConfig {
    pub history: bool,
    pub online_docs: bool,
    pub fuzzy: bool,
    /// Predict the next token proactively at untyped positions (after a
    /// trailing space, and on an empty prompt) instead of waiting for a
    /// typed prefix.
    pub predictive: bool,
    /// Complete to entire remembered command lines (`c` ->
    /// `claude --dangerously-skip-permissions`), not just single tokens.
    pub chain_prediction: bool,
    /// Half-life in days for recency weighting: recent repetition outweighs
    /// old habits without erasing them from memory.
    pub half_life_days: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub ghost_text: bool,
    pub max_candidates: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProvidersConfig {
    pub forge: ProviderToggle,
    pub docker: ProviderToggle,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderToggle {
    pub enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CarapaceConfig {
    /// Consult carapace when it is found on PATH. Adaptive stays fully
    /// functional without it; this only gates the extra source.
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
            predictive: true,
            chain_prediction: true,
            half_life_days: 21.0,
        }
    }
}
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            ghost_text: true,
            max_candidates: 8,
        }
    }
}
impl Default for ProviderToggle {
    fn default() -> Self {
        Self { enabled: true }
    }
}
impl Default for CarapaceConfig {
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
    /// Persists the configuration atomically (temp file + rename) at its
    /// configured path. Used by the CLI toggles
    /// (`adaptive carapace enable|disable`).
    pub fn save(&self) -> Result<()> {
        self.save_to(Self::path())
    }
    pub fn save_to(&self, path: PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let raw = toml::to_string_pretty(self).context("could not serialize config")?;
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        fs::write(&temp, raw).with_context(|| format!("could not write {}", temp.display()))?;
        fs::rename(&temp, &path).with_context(|| format!("could not update {}", path.display()))?;
        Ok(())
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
        assert!(Config::default().providers.forge.enabled);
        assert!(Config::default().carapace.enabled);
    }
    #[test]
    fn save_roundtrips_the_toggle() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("nested").join("config.toml");
        let mut config = Config::default();
        config.carapace.enabled = false;
        config.save_to(path.clone()).unwrap();
        let reloaded: Config = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(!reloaded.carapace.enabled);
        assert!(reloaded.completion.history);
    }
}
