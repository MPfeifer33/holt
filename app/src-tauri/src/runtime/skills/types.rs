use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How a skill is delivered to the agent's context.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SkillMode {
    /// Always injected into system prompt when active (best for short directives).
    Inject,
    /// Available on-demand — agent sees a one-line manifest entry and can pull full content.
    Reference,
    /// System decides based on token count threshold (default behavior).
    #[default]
    Auto,
}

/// Hard threshold for auto mode: skills at or below this token count inject,
/// above this become reference-mode.
pub const AUTO_MODE_THRESHOLD: usize = 400;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SkillPriority {
    Low,
    #[default]
    Normal,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerCondition {
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub always: bool,
    #[serde(default)]
    pub manual: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub triggers: Vec<TriggerCondition>,
    #[serde(default)]
    pub priority: SkillPriority,
    #[serde(default)]
    pub mode: SkillMode,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,
    #[serde(default)]
    pub includes: Option<Vec<String>>,
}

fn default_max_tokens() -> usize {
    1000
}

#[derive(Debug, Clone)]
pub struct SkillDefinition {
    pub file_name: String,
    pub frontmatter: SkillFrontmatter,
    pub body: String,
    pub file_path: PathBuf,
    pub is_directory: bool,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SkillSource {
    AgentAssigned,
    AutoTriggered,
}

/// The resolved delivery mode after applying auto-threshold logic.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryMode {
    Inject,
    Reference,
}

#[derive(Debug, Clone)]
pub struct ResolvedSkill {
    pub definition: SkillDefinition,
    pub source: SkillSource,
    pub delivery: DeliveryMode,
}

impl SkillDefinition {
    /// Determine effective delivery mode, resolving `Auto` based on token threshold.
    pub fn effective_mode(&self) -> DeliveryMode {
        match &self.frontmatter.mode {
            SkillMode::Inject => DeliveryMode::Inject,
            SkillMode::Reference => DeliveryMode::Reference,
            SkillMode::Auto => {
                if self.frontmatter.max_tokens <= AUTO_MODE_THRESHOLD {
                    DeliveryMode::Inject
                } else {
                    DeliveryMode::Reference
                }
            }
        }
    }
}

/// Context provided to the resolver to determine which skills to activate.
pub struct SkillContext {
    pub agent_skill_names: Vec<String>,
    pub recent_tool_names: Vec<String>,
    pub recent_file_paths: Vec<String>,
    pub last_user_message: String,
    pub token_budget: usize,
}
