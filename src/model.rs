use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub value: String,
    pub display: String,
    pub description: String,
    pub source: Source,
    pub confidence: f64,
    #[serde(default)]
    pub score: f64,
}

impl Candidate {
    pub fn new(value: impl Into<String>, description: impl Into<String>, source: Source) -> Self {
        let value = value.into();
        Self {
            display: value.clone(),
            value,
            description: description.into(),
            confidence: source.base_confidence(),
            score: 0.0,
            source,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Native,
    Dynamic,
    LocalHelp,
    OfficialDocs,
    Filesystem,
    History,
}

impl Source {
    pub fn base_confidence(self) -> f64 {
        match self {
            Self::Native => 0.98,
            Self::Dynamic => 0.94,
            Self::OfficialDocs => 0.91,
            Self::LocalHelp => 0.88,
            Self::Filesystem => 0.90,
            Self::History => 0.55,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct CommandSchema {
    pub command: String,
    pub path: Vec<String>,
    pub usage: Option<String>,
    pub items: Vec<SchemaItem>,
    pub confidence: f64,
    pub executable_fingerprint: String,
    pub discovered_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchemaItem {
    pub names: Vec<String>,
    pub kind: ItemKind,
    pub value_hint: Option<String>,
    pub values: Vec<String>,
    pub description: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Flag,
    Subcommand,
    Positional,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryContext {
    pub buffer: String,
    pub cursor: usize,
    pub tokens: Vec<String>,
    pub current: String,
    pub command: Option<String>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResponse {
    pub request_id: u64,
    pub prefix_len: usize,
    pub candidates: Vec<Candidate>,
    pub cache_only: bool,
}
