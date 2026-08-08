use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// RGB color for agent node rendering. Alpha is always 1.0 — opacity is
/// controlled by the constellation renderer based on agent status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Default for AgentColor {
    fn default() -> Self {
        Self {
            r: 100,
            g: 149,
            b: 237,
        } // Cornflower blue
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentStatus {
    Idle,
    Working,
    WaitingForHil,
    WaitingForAgent(String),
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// A single content block in a multimodal message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        /// Base64-encoded image data
        data: String,
        /// MIME type: "image/png", "image/jpeg", "image/gif", "image/webp"
        media_type: String,
    },
}

/// Image attachment from the frontend (paste/drop).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAttachment {
    pub data: String,
    pub media_type: String,
    pub filename: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub role: MessageRole,
    pub content: String,
    /// Multimodal content blocks. When Some, this is the full payload sent to the API.
    /// The `content` field becomes a text-only summary for display/logging.
    #[serde(default)]
    pub content_blocks: Option<Vec<ContentBlock>>,
    pub timestamp: DateTime<Utc>,
    pub token_estimate: Option<u32>,
    pub tool_calls: Option<Vec<ToolCallRecord>>,
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub outcome_tag: Option<OutcomeTag>,
    /// Extended thinking content (Anthropic protocol only)
    #[serde(default)]
    pub thinking_content: Option<String>,
    /// OpenAI Responses API phase tag for assistant messages.
    /// `"commentary"` = intermediate assistant update / preamble before a
    /// tool call. `"final_answer"` = completed final response.
    /// Populated from Responses stream events; replayed in subsequent turn
    /// inputs to preserve the model's intended message flow. None for
    /// non-Responses agents and for non-assistant messages.
    ///
    #[serde(default)]
    pub responses_phase: Option<String>,
    /// OpenAI Responses API reasoning items (chain-of-thought).
    /// Opaque JSON preserved across turns so the model doesn't re-reason from
    /// scratch on each new prompt. None for non-Responses agents.
    ///
    #[serde(default)]
    pub reasoning_items: Option<serde_json::Value>,
}

/// User-applied outcome tag for HIL failure tracking.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum OutcomeTag {
    Success,
    Failure,
    Partial,
}

/// Runtime settings for Anthropic-protocol agents (not persisted).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicSettings {
    pub model: String,
    pub thinking_enabled: bool,
    pub thinking_budget: u32,
    pub temperature: f32,
}

impl Default for AnthropicSettings {
    fn default() -> Self {
        Self {
            model: "claude-opus-4-6".to_string(),
            thinking_enabled: true,
            thinking_budget: 16384,
            temperature: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConversationHistory {
    pub messages: Vec<ConversationMessage>,
    pub total_token_estimate: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadata {
    pub total_tokens_used: u64,
    pub total_api_calls: u64,
    pub average_latency_ms: f64,
    pub created_at: DateTime<Utc>,
    pub last_active: Option<DateTime<Utc>>,
    /// Additional API calls granted beyond the base limit via extend_limit.
    /// Limit check: total_api_calls < (base_limit + limit_extension).
    #[serde(default)]
    pub limit_extension: u64,
}

impl Default for AgentMetadata {
    fn default() -> Self {
        Self {
            total_tokens_used: 0,
            total_api_calls: 0,
            average_latency_ms: 0.0,
            created_at: Utc::now(),
            last_active: None,
            limit_extension: 0,
        }
    }
}

/// Placeholder for tool definitions — fully implemented in Session 5.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Parameters captured after each successful API call for cache-efficient fork spawning.
/// When a fork is spawned, these exact bytes are reused so providers with prompt caching
/// (Anthropic) only charge for new tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheSafeParams {
    pub system_prompt: String,
    pub tool_definitions: Vec<serde_json::Value>,
    pub context_messages: Arc<Vec<ConversationMessage>>,
}

// SubagentSlot is now fully defined in runtime::subagent (Session 6).
pub use super::subagent::SubagentSlot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSlot {
    pub id: String,
    pub name: String,
    pub color: AgentColor,
    /// Protected agents cannot be deleted through the UI or API.
    #[serde(default)]
    pub protected: bool,
    pub connection: super::connection::ConnectionConfig,
    pub protocol: super::connection::AgentProtocol,
    pub working_directory: PathBuf,
    pub status: AgentStatus,
    pub subagents: Vec<SubagentSlot>,
    pub conversation: ConversationHistory,
    pub tools: Vec<ToolDefinition>,
    pub system_prompt: String,
    pub metadata: AgentMetadata,
    /// Per-agent execution limits (configurable via agent TOML)
    #[serde(default)]
    pub limits: super::limits::AgentLimits,
    /// Model context window size in tokens (used for context budget calculation)
    #[serde(default = "default_context_window")]
    pub context_window_size: u32,
    /// Whether the user explicitly set context_tokens in agent config (skips cloud probe)
    #[serde(default)]
    pub context_tokens_override: Option<u32>,
    /// Whether the cloud API probe has already run this session (prevents re-probing on every message)
    #[serde(skip)]
    pub context_probed: bool,
    /// Per-agent sandbox configuration (loaded from agent TOML at startup)
    #[serde(skip)]
    pub sandbox_config: crate::config::agent_config::BuildSandboxBlock,
    /// Active sandbox state (test-fix loop). Not serialized directly —
    /// persisted through AgentSession for crash recovery.
    #[serde(skip)]
    pub sandbox: Option<super::sandbox::SandboxState>,
    /// Cumulative prompt tokens for this session (actual from API responses)
    #[serde(default)]
    pub session_prompt_tokens: u64,
    /// Cumulative completion tokens for this session (actual from API responses)
    #[serde(default)]
    pub session_completion_tokens: u64,
    /// Most recent prompt token count (= current context window usage for SDK agents)
    #[serde(default)]
    pub current_context_tokens: u64,
    /// When the first message was sent in this session (persists across restarts)
    #[serde(default)]
    pub session_start: Option<DateTime<Utc>>,
    /// Dirty flag for incremental auto-save
    #[serde(skip)]
    pub is_dirty: bool,
    /// Runtime settings for Anthropic-protocol agents (model, thinking)
    #[serde(skip)]
    pub anthropic_settings: Option<AnthropicSettings>,
    /// Skill file-stem names assigned to this agent
    #[serde(default)]
    pub skills: Vec<String>,
    /// User notes injected into system prompt via /btw command.
    /// Persistent across turns — agent sees these as background context.
    #[serde(default)]
    pub user_notes: Vec<String>,
    /// Provider-managed session/thread ID persisted across restarts for continuity.
    /// Used by the Anthropic SDK lane and the Codex lane.
    #[serde(default, alias = "claude_session_id")]
    pub agent_session_id: Option<String>,
    /// Hash of system prompt when session was created — invalidates session if persona changes.
    #[serde(default)]
    pub system_prompt_hash: Option<String>,
    /// When true, the spawn_subagent tool is removed and a coordinator preamble
    /// is prepended to the system prompt (auto-set for multi-agent models).
    #[serde(default)]
    pub coordinator_mode: bool,
    /// When true, SDK tools auto-approve in canUseTool callback (no round-trip to Rust).
    /// Temporary until per-agent tool permissions system is built.
    #[serde(default)]
    pub autonomous: bool,
    /// Whether pre-emptive memory extraction has run this compaction cycle.
    /// Reset when compaction is detected (context token drop > threshold).
    #[serde(skip)]
    pub extraction_pending_done: bool,
    /// Completion provider for HTTP-based agents (None for SDK/MCP).
    /// Constructed by `providers::build_provider()` on agent creation.
    #[serde(skip)]
    pub provider: Option<std::sync::Arc<dyn crate::providers::types::CompletionProvider>>,
    /// Cache-safe parameters from last successful API call.
    /// Used for fork context inheritance. Cleared on connection/model/tool/prompt changes.
    #[serde(skip)]
    pub cached_params: Option<CacheSafeParams>,
    /// Whether this agent runs multi-agent models (e.g., Grok 4.20 swarm).
    /// When true, forking is rejected.
    #[serde(default)]
    pub multi_agent_model: bool,
    /// Cognitive base archetype (e.g., "scout", "sentinel"). Immutable after creation.
    #[serde(default)]
    pub archetype_base: Option<String>,
    /// Role specialization (e.g., "code_reviewer", "red_teamer"). Immutable after creation.
    #[serde(default)]
    pub archetype_specialization: Option<String>,
    #[serde(skip)]
    pub session_memory: super::session_memory::SessionMemoryState,
    #[serde(skip)]
    pub cognitive_intake: super::cognitive_intake::CognitiveIntakeState,
    #[serde(default)]
    pub appearance: Option<crate::runtime::appearance::AgentAppearance>,
    /// Runtime settings for agents using OpenAI's Responses API (GPT-5 family).
    /// Populated only for agents whose connection endpoint routes to
    /// `api.openai.com`. None for every other agent (Chat Completions
    /// compatible endpoints, Anthropic, SDK, MCP, etc.).
    ///
    #[serde(default)]
    pub openai_responses_settings:
        Option<crate::runtime::openai_responses_settings::OpenAIResponsesSettings>,
    /// Runtime settings for agents using xAI Grok models.
    /// Populated only for agents whose connection endpoint routes to
    /// `api.x.ai`. None for every other agent.
    #[serde(default)]
    pub xai_settings: Option<crate::runtime::xai_settings::XAISettings>,
    /// Runtime settings for agents using Google Gemini models.
    /// Populated only for agents whose connection endpoint routes to
    /// `generativelanguage.googleapis.com`. None for every other agent.
    #[serde(default)]
    pub gemini_settings: Option<crate::runtime::gemini_settings::GeminiSettings>,
    /// Runtime settings for agents using local models (Ollama, LM Studio, etc.).
    /// Populated only for agents with a Local connection config. None for every
    /// other agent.
    #[serde(default)]
    pub local_model_settings: Option<crate::runtime::local_model_settings::LocalModelSettings>,
}

fn default_context_window() -> u32 {
    125_000
}

/// Maximum number of messages before oldest non-system messages are trimmed.
const MAX_MESSAGES: usize = 10_000;

impl AgentSlot {
    pub fn add_message(&mut self, message: ConversationMessage) {
        // Update rough token estimate: characters / 4 as a heuristic
        let char_count =
            message.content.len() + message.thinking_content.as_ref().map_or(0, |t| t.len());
        let token_estimate = (char_count / 4) as u32;
        self.conversation.total_token_estimate = self
            .conversation
            .total_token_estimate
            .saturating_add(token_estimate);

        self.conversation.messages.push(message);

        // Trim oldest non-system messages if we exceed the cap
        if self.conversation.messages.len() > MAX_MESSAGES {
            let trim_count = self.conversation.messages.len() - MAX_MESSAGES;
            tracing::warn!(
                agent_id = %self.id,
                trim_count,
                "Conversation exceeded {} messages, trimming oldest non-system messages",
                MAX_MESSAGES,
            );
            let mut removed = 0;
            self.conversation.messages.retain(|m| {
                if removed >= trim_count {
                    return true;
                }
                if m.role == MessageRole::System {
                    return true;
                }
                removed += 1;
                false
            });
        }

        self.is_dirty = true;
    }

    /// Check turn limit using the agent's own limits configuration.
    /// Increments the counter and checks against the per-agent limit.
    pub(crate) fn limits_check_and_increment(&mut self) -> super::limits::LimitCheckResult {
        self.limits.increment_tool_calls();

        // Check session limit first (more severe)
        if self.limits.check_session_limit() == super::limits::LimitCheckResult::SessionLimitReached
        {
            return super::limits::LimitCheckResult::SessionLimitReached;
        }

        self.limits.check_turn_limit()
    }

    /// Clear cached parameters — called when connection, model, tools, or system prompt changes.
    pub fn invalidate_cached_params(&mut self) {
        self.cached_params = None;
    }

    /// Reset turn counter (called at start of each new user message).
    pub(crate) fn reset_turn_limits(&mut self) {
        self.limits.reset_turn_counter();
    }

    /// True when this slot's turns are driven by an external process (e.g. the
    /// Claude Code TUI lane) rather than Holt's turn machinery. Holt
    /// must never submit engine turns on its behalf — port 0 is the placeholder
    /// connection used by such slots.
    pub fn is_externally_driven(&self) -> bool {
        matches!(
            &self.connection,
            crate::runtime::connection::ConnectionConfig::Local { port: 0, .. }
        )
    }
}

/// Best-effort heuristic for whether a model supports image input.
pub fn model_supports_vision(model: &str) -> bool {
    let lower = model.to_lowercase();
    // Claude 3+ models
    lower.contains("claude-3") || lower.contains("claude-opus")
        || lower.contains("claude-sonnet") || lower.contains("claude-haiku")
    // OpenAI vision models
        || lower.contains("gpt-4o") || lower.contains("gpt-4-vision") || lower.contains("gpt-4-turbo")
    // Grok vision models
        || lower.contains("grok-2-vision") || lower.contains("grok-4")
    // Local multimodal models
        || lower.contains("llava") || lower.contains("bakllava") || lower.contains("moondream")
}

/// Check if a model name indicates internal multi-agent capabilities.
pub fn is_multi_agent_model(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.contains("grok-4.20-multi-agent")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_block_serde_roundtrip_text() {
        let block = ContentBlock::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        match back {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn test_content_block_serde_roundtrip_image() {
        let block = ContentBlock::Image {
            data: "iVBOR".to_string(),
            media_type: "image/png".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        match back {
            ContentBlock::Image { data, media_type } => {
                assert_eq!(data, "iVBOR");
                assert_eq!(media_type, "image/png");
            }
            _ => panic!("expected Image"),
        }
    }

    #[test]
    fn test_conversation_message_without_content_blocks_deserializes() {
        let json = r#"{"role":"User","content":"hello","timestamp":"2026-01-01T00:00:00Z","token_estimate":null,"tool_calls":null,"tool_call_id":null,"outcome_tag":null,"thinking_content":null}"#;
        let msg: ConversationMessage = serde_json::from_str(json).unwrap();
        assert!(msg.content_blocks.is_none());
    }

    #[test]
    fn test_model_supports_vision() {
        assert!(model_supports_vision("claude-sonnet-4-20250514"));
        assert!(model_supports_vision("gpt-4o-mini"));
        assert!(model_supports_vision("grok-4"));
        assert!(model_supports_vision("llava:13b"));
        assert!(!model_supports_vision("llama3"));
        assert!(!model_supports_vision("mistral-7b"));
    }

    #[test]
    fn test_is_multi_agent_model() {
        assert!(is_multi_agent_model("grok-4.20-multi-agent-0309"));
        assert!(is_multi_agent_model("grok-4.20-multi-agent-beta-0309"));
        assert!(is_multi_agent_model("grok-4.20-multi-agent"));
        assert!(is_multi_agent_model("Grok-4.20-Multi-Agent-Latest"));
        assert!(!is_multi_agent_model("grok-3-latest"));
        assert!(!is_multi_agent_model("claude-opus-4-6"));
        assert!(!is_multi_agent_model("qwen3-coder"));
    }

    #[test]
    fn test_cached_params_lifecycle() {
        let mut agent = crate::runtime::engine::test_utils::test_agent();
        assert!(agent.cached_params.is_none());

        agent.cached_params = Some(CacheSafeParams {
            system_prompt: "test prompt".to_string(),
            tool_definitions: vec![serde_json::json!({"name": "bash"})],
            context_messages: Arc::new(vec![]),
        });
        assert!(agent.cached_params.is_some());
        assert_eq!(
            agent.cached_params.as_ref().unwrap().system_prompt,
            "test prompt"
        );

        agent.invalidate_cached_params();
        assert!(agent.cached_params.is_none());
    }

    #[test]
    fn test_cache_safe_params_uses_arc() {
        let messages = vec![];
        let params = CacheSafeParams {
            system_prompt: "test".to_string(),
            tool_definitions: vec![],
            context_messages: Arc::new(messages),
        };
        // Arc clone is a refcount bump, not a deep copy
        let clone = params.context_messages.clone();
        assert!(Arc::ptr_eq(&params.context_messages, &clone));
    }

    #[test]
    fn test_multi_agent_model_default_false() {
        let agent = crate::runtime::engine::test_utils::test_agent();
        assert!(!agent.multi_agent_model);
    }

    #[test]
    fn tui_placeholder_slot_is_externally_driven() {
        let mut slot = crate::runtime::engine::test_utils::test_agent();
        slot.connection = crate::runtime::connection::ConnectionConfig::Local {
            host: "127.0.0.1".into(),
            port: 0,
            model: None,
        };
        assert!(slot.is_externally_driven());
        slot.connection = crate::runtime::connection::ConnectionConfig::Local {
            host: "127.0.0.1".into(),
            port: 8080,
            model: None,
        };
        assert!(!slot.is_externally_driven());
    }
}
