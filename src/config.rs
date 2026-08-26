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

/// How far a fuzzy match may sprawl: the largest gap of unmatched
/// characters allowed between two consecutive typed characters when the
/// prefix is matched as an in-order subsequence. Mirrors the familiar
/// tight / smart / loose ladder other suggesters expose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FuzzyPreset {
    Tight,
    #[default]
    Smart,
    Loose,
}

impl FuzzyPreset {
    /// Maximum unmatched characters permitted between consecutive typed
    /// characters (tight ≈ near-adjacent, loose = forgiving).
    pub fn max_gap(self) -> usize {
        match self {
            Self::Tight => 1,
            Self::Smart => 4,
            Self::Loose => 8,
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "tight" => Some(Self::Tight),
            "smart" => Some(Self::Smart),
            "loose" => Some(Self::Loose),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tight => "tight",
            Self::Smart => "smart",
            Self::Loose => "loose",
        }
    }
    /// The preset that follows `self` cycling tight -> smart -> loose.
    pub fn next(self) -> Self {
        match self {
            Self::Tight => Self::Smart,
            Self::Smart => Self::Loose,
            Self::Loose => Self::Tight,
        }
    }
    /// The preset before `self` cycling loose <- smart <- tight.
    pub fn previous(self) -> Self {
        match self {
            Self::Tight => Self::Loose,
            Self::Smart => Self::Tight,
            Self::Loose => Self::Smart,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompletionConfig {
    pub history: bool,
    pub online_docs: bool,
    pub fuzzy: bool,
    /// Sprawl bound for subsequence ("fuzzy") matches; see [`FuzzyPreset`].
    /// Stored as a string so hand-edited configs stay readable.
    pub fuzzy_preset: String,
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
    /// Predict whole commands on an untouched prompt (before any key is
    /// pressed). Persisted global toggle behind `adaptive empty on|off`.
    pub empty_prompt_predictions: bool,
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
            fuzzy_preset: FuzzyPreset::default().as_str().to_owned(),
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
            empty_prompt_predictions: true,
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
        let mut config = Self::load_file()?;
        // Environment overrides beat the saved file, so a single shell can
        // experiment without rewriting global state (the daemon reads the
        // saved file; sessions that exported a value keep theirs).
        if let Ok(preset) = std::env::var("ADAPTIVE_FUZZY")
            && FuzzyPreset::parse(&preset).is_some()
        {
            config.completion.fuzzy = true;
            config.completion.fuzzy_preset = preset.trim().to_ascii_lowercase();
        }
        match std::env::var("ADAPTIVE_EMPTY").as_deref() {
            Ok("on") | Ok("1") => config.ui.empty_prompt_predictions = true,
            Ok("off") | Ok("0") => config.ui.empty_prompt_predictions = false,
            _ => {}
        }
        Ok(config)
    }
    fn load_file() -> Result<Self> {
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
    /// The configured fuzzy preset, falling back to the default when the
    /// stored string is unrecognized.
    pub fn fuzzy_preset(&self) -> FuzzyPreset {
        FuzzyPreset::parse(&self.completion.fuzzy_preset).unwrap_or_default()
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
        assert_eq!(reloaded.completion.fuzzy_preset, "smart");
        assert!(reloaded.ui.empty_prompt_predictions);
    }

    #[test]
    fn fuzzy_preset_parses_and_cycles_both_ways() {
        use FuzzyPreset::*;
        assert_eq!(
            [(Tight, Smart), (Smart, Loose), (Loose, Tight)].map(|(a, _)| a.next()),
            [Smart, Loose, Tight]
        );
        assert_eq!(
            [(Smart, Tight), (Loose, Smart), (Tight, Loose)].map(|(a, _)| a.previous()),
            [Tight, Smart, Loose]
        );
        assert_eq!(FuzzyPreset::parse("TIGHT"), Some(Tight));
        assert_eq!(FuzzyPreset::parse("nope"), None);
        assert_eq!(Config::default().fuzzy_preset(), Smart);
    }
}
