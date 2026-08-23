use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub value: String,
    pub display: String,
    pub description: String,
    pub source: Source,
    pub confidence: f64,
    /// What this token *is* at this position. Ranking needs it to answer
    /// "what would someone type here" rather than "what strings match": a
    /// subcommand slot wants subcommands first and flags after, whatever the
    /// alphabet says.
    #[serde(default)]
    pub kind: CandidateKind,
    /// How commonly this token is used here by anyone, in 0.0..=1.0. See
    /// `crate::priors`; 0.0 simply means "no global opinion".
    #[serde(default)]
    pub prior: f64,
    #[serde(default)]
    pub score: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    /// An argument value: a path, a branch, a container, a script name.
    #[default]
    Value,
    Subcommand,
    Flag,
}

impl Candidate {
    pub fn new(value: impl Into<String>, description: impl Into<String>, source: Source) -> Self {
        let value = value.into();
        let kind = if value.starts_with('-') {
            CandidateKind::Flag
        } else {
            CandidateKind::Value
        };
        Self {
            display: value.clone(),
            value,
            description: description.into(),
            confidence: source.base_confidence(),
            kind,
            prior: 0.0,
            score: 0.0,
            source,
        }
    }
    pub fn with_kind(mut self, kind: CandidateKind) -> Self {
        self.kind = kind;
        self
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
    Command,
    /// A well-known continuation from the global prior table, offered when
    /// discovery could not read the tool's own metadata (see `crate::priors`).
    Prior,
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
            Self::Command => 0.75,
            // Curated metadata about a real CLI, on a par with that CLI's own
            // help text -- just a notch below it, so a name we actually read
            // out of the tool wins a tie against a name we only expected.
            Self::Prior => 0.86,
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
    pub raw_current_len: usize,
    pub command: Option<String>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResponse {
    pub request_id: u64,
    pub prefix_len: usize,
    pub candidates: Vec<Candidate>,
    pub cache_only: bool,
    /// Identifies the completion position this response describes, so
    /// frontends can send accept/reject feedback back to the exact same
    /// context without re-deriving engine internals. Empty only for
    /// degenerate positions.
    #[serde(default)]
    pub context_key: String,
}
