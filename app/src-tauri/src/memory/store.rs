use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Type definitions (used by tools and engine)
// ---------------------------------------------------------------------------

/// Category of memory content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCategory {
    Episode,
    Concept,
    Policy,
    Identity,
}

impl MemoryCategory {
    /// Parse the `category` argument of a memory tool call.
    ///
    /// The single place a tool argument becomes a category. `remember` used to carry its
    /// own inline match — a fifth copy of the category mapping — and unknown values fall
    /// back to `Episode` here as they did there.
    ///
    /// `state` was dropped 2026-07-26. It had always mapped to `MemoryKind::Observation`,
    /// identically to `episode`, and category is derived from kind on read — so no memory
    /// ever recorded the distinction and nothing is lost. Old callers passing "state" land
    /// on `Episode`, which is exactly where they landed before.
    pub fn from_tool_arg(raw: Option<&str>) -> Self {
        match raw {
            Some("concept") => Self::Concept,
            Some("policy") => Self::Policy,
            Some("identity") => Self::Identity,
            _ => Self::Episode,
        }
    }
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Episode => write!(f, "episode"),
            Self::Concept => write!(f, "concept"),
            Self::Policy => write!(f, "policy"),
            Self::Identity => write!(f, "identity"),
        }
    }
}

/// Temperature tier controlling retrieval priority and TTL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTier {
    Hot,
    Warm,
    Cold,
    Archive,
}

impl std::fmt::Display for MemoryTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hot => write!(f, "hot"),
            Self::Warm => write!(f, "warm"),
            Self::Cold => write!(f, "cold"),
            Self::Archive => write!(f, "archive"),
        }
    }
}

/// How the memory was created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    /// Human explicitly marked something.
    HumanStated,
    /// Agent stored a memory via the remember tool.
    AgentExplicit,
    /// Passive extraction from conversation (cognitive intake system).
    CognitiveIntake,
    /// Auto-capture from conversation analysis.
    AutoCaptured,
}

impl std::fmt::Display for MemorySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HumanStated => write!(f, "human_stated"),
            Self::AgentExplicit => write!(f, "agent_explicit"),
            Self::CognitiveIntake => write!(f, "cognitive_intake"),
            Self::AutoCaptured => write!(f, "auto_captured"),
        }
    }
}

/// Input for storing a new memory.
#[derive(Clone)]
pub struct StoreInput {
    pub content: String,
    pub category: MemoryCategory,
    pub importance: f64,
    pub source: MemorySource,
    pub agent_ns: String,
    pub tier: MemoryTier,
    pub tags: Vec<String>,
}
