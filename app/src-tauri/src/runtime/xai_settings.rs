// app/src-tauri/src/runtime/xai_settings.rs
//
// Runtime settings for agents using xAI's Grok models.
//
// Parallel to `runtime::openai_responses_settings::OpenAIResponsesSettings` —
// each agent can have its own xAI configuration stored on its `AgentSlot`.
// The settings are persisted via `persistence::session::AgentSession` so user
// preferences survive restarts.
//
// xAI offers several unique capabilities beyond the standard OpenAI-compatible
// interface:
//
// - **Cache affinity** via the `x-grok-conv-id` header: a stable UUID per
//   session tells xAI's servers to reuse cached KV state across turns.
// - **Server-side search**: Grok can perform web and X (Twitter) searches
//   natively, returning citations alongside completions.
// - **Deferred completions**: Fire-and-forget mode for long tasks — the API
//   returns a `request_id` immediately, and the result can be polled later.
// - **Always-on reasoning**: grok-4.3 and grok-4 always reason (no toggle).
//   Sampling params (temperature, top_p) are incompatible with these models.
// - **`cost_in_usd_ticks`**: Precise billing info in the response usage block.
//
use serde::{Deserialize, Serialize};

/// Per-agent runtime configuration for xAI Grok models.
///
/// Stored on `AgentSlot` as an `Option` so agents that don't use xAI have zero
/// footprint. Populated when an agent's connection endpoint resolves to
/// `api.x.ai` and the agent is created or configured.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XAISettings {
    /// Model name (e.g., "grok-4.3", "grok-4.1-fast", "grok-4", "grok-4-fast",
    /// "grok-4.20-multi-agent").
    pub model: String,

    /// Stable UUID for cache affinity. Sent as the `x-grok-conv-id` header on
    /// every request. Generated on session start, reused across turns within
    /// the same session. xAI uses this to reuse cached KV state, giving 75-90%
    /// cache discounts on input tokens.
    pub conversation_id: Option<String>,

    /// Server-side web/X search mode. When enabled, Grok performs searches
    /// and includes citations in its response.
    pub search_mode: SearchMode,

    /// Fire-and-forget mode. When true, the API returns a `request_id`
    /// immediately without waiting for completion. The result must be polled
    /// later via the deferred completions endpoint.
    pub deferred: bool,

    /// Reasoning effort level. Only meaningful for multi-agent models.
    /// Always-reasoning models (grok-4.3, grok-4) ignore this.
    pub reasoning_effort: Option<ReasoningEffort>,

    /// Maximum completion tokens. `None` means use model default.
    pub max_completion_tokens: Option<u32>,

    /// Sampling temperature (0.0 – 2.0). Only sent for models that are NOT
    /// always-reasoning. Always-reasoning models (grok-4.3, grok-4) reject
    /// this parameter.
    pub temperature: Option<f32>,
}

/// Server-side search mode for xAI Grok models.
///
/// When enabled, Grok performs web and X (Twitter) searches as part of
/// generating its response, and includes citation metadata in the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// No server-side search. Default.
    Off,
    /// Always search before responding.
    On,
    /// Let the model decide whether to search based on the query.
    Auto,
}

impl SearchMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SearchMode::Off => "off",
            SearchMode::On => "on",
            SearchMode::Auto => "auto",
        }
    }

    /// Whether this mode should include `search_parameters` in the request body.
    pub fn should_include(&self) -> bool {
        !matches!(self, SearchMode::Off)
    }
}

impl Default for SearchMode {
    fn default() -> Self {
        Self::Off
    }
}

/// Reasoning effort levels for xAI multi-agent models.
///
/// Only applicable to `*-multi-agent` models. Always-reasoning models
/// (grok-4.3, grok-4) don't accept a reasoning_effort parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }
}

impl Default for ReasoningEffort {
    fn default() -> Self {
        Self::Medium
    }
}

impl XAISettings {
    /// Returns true if this model uses always-on reasoning (no toggle, no
    /// sampling params). Currently grok-4.3 and grok-4 (non-fast variants).
    pub fn is_always_reasoning(&self) -> bool {
        is_always_reasoning_model(&self.model)
    }

    /// Returns true if this is a multi-agent model that uses the Responses API
    /// endpoint and doesn't support client-side tools.
    pub fn is_multi_agent(&self) -> bool {
        self.model.contains("multi-agent")
    }

    /// Returns true if sampling parameters (temperature, top_p) are allowed
    /// for this model. Always-reasoning models reject these.
    pub fn allows_sampling_params(&self) -> bool {
        !self.is_always_reasoning()
    }

    /// Context window size in tokens for the configured model.
    pub fn context_window(&self) -> u32 {
        context_window_for_model(&self.model)
    }
}

/// Check if a model name corresponds to an always-reasoning Grok model.
pub fn is_always_reasoning_model(model: &str) -> bool {
    // grok-4.3 and grok-4 (exact, not grok-4-fast or grok-4.1-fast) use
    // always-on reasoning. The "fast" variants do NOT.
    model == "grok-4.3" || model == "grok-4"
}

/// Get the context window size for a given Grok model.
fn context_window_for_model(model: &str) -> u32 {
    match model {
        "grok-4.3" => 1_048_576,                       // 1M
        "grok-4" => 131_072,                           // 128K
        "grok-4-fast" => 131_072,                      // 128K
        "grok-4.1-fast" => 2_097_152,                  // 2M
        _ if model.contains("multi-agent") => 131_072, // 128K default
        _ => 131_072,                                  // Conservative default
    }
}

impl Default for XAISettings {
    /// Defaults:
    /// - `model`: "grok-4.3" (flagship reasoning model)
    /// - `conversation_id`: `None` (generated on first session start)
    /// - `search_mode`: `Off`
    /// - `deferred`: `false`
    /// - `reasoning_effort`: `None` (not applicable for default model)
    /// - `max_completion_tokens`: `None` (model default)
    /// - `temperature`: `None` (model default)
    fn default() -> Self {
        Self {
            model: "grok-4.3".to_string(),
            conversation_id: None,
            search_mode: SearchMode::default(),
            deferred: false,
            reasoning_effort: None,
            max_completion_tokens: None,
            temperature: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let s = XAISettings::default();
        assert_eq!(s.model, "grok-4.3");
        assert!(s.conversation_id.is_none());
        assert_eq!(s.search_mode, SearchMode::Off);
        assert!(!s.deferred);
        assert!(s.reasoning_effort.is_none());
        assert!(s.max_completion_tokens.is_none());
        assert!(s.temperature.is_none());
    }

    #[test]
    fn test_always_reasoning_detection() {
        let mut s = XAISettings::default();
        s.model = "grok-4.3".into();
        assert!(s.is_always_reasoning());
        assert!(!s.allows_sampling_params());

        s.model = "grok-4".into();
        assert!(s.is_always_reasoning());

        s.model = "grok-4-fast".into();
        assert!(!s.is_always_reasoning());
        assert!(s.allows_sampling_params());

        s.model = "grok-4.1-fast".into();
        assert!(!s.is_always_reasoning());
    }

    #[test]
    fn test_multi_agent_detection() {
        let mut s = XAISettings::default();
        s.model = "grok-4.20-multi-agent".into();
        assert!(s.is_multi_agent());
        assert!(!s.is_always_reasoning());

        s.model = "grok-4.3".into();
        assert!(!s.is_multi_agent());
    }

    #[test]
    fn test_context_window() {
        let mut s = XAISettings::default();
        s.model = "grok-4.3".into();
        assert_eq!(s.context_window(), 1_048_576);

        s.model = "grok-4.1-fast".into();
        assert_eq!(s.context_window(), 2_097_152);

        s.model = "grok-4".into();
        assert_eq!(s.context_window(), 131_072);
    }

    #[test]
    fn test_search_mode_serde_roundtrip() {
        for mode in [SearchMode::Off, SearchMode::On, SearchMode::Auto] {
            let json = serde_json::to_string(&mode).unwrap();
            let back: SearchMode = serde_json::from_str(&json).unwrap();
            assert_eq!(back, mode, "round-trip failed for {:?}", mode);
        }
    }

    #[test]
    fn test_search_mode_as_str() {
        assert_eq!(SearchMode::Off.as_str(), "off");
        assert_eq!(SearchMode::On.as_str(), "on");
        assert_eq!(SearchMode::Auto.as_str(), "auto");
    }

    #[test]
    fn test_search_mode_should_include() {
        assert!(!SearchMode::Off.should_include());
        assert!(SearchMode::On.should_include());
        assert!(SearchMode::Auto.should_include());
    }

    #[test]
    fn test_reasoning_effort_serde_roundtrip() {
        for effort in [
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ] {
            let json = serde_json::to_string(&effort).unwrap();
            let back: ReasoningEffort = serde_json::from_str(&json).unwrap();
            assert_eq!(back, effort);
        }
    }

    #[test]
    fn test_full_settings_serde_roundtrip() {
        let s = XAISettings {
            model: "grok-4.1-fast".to_string(),
            conversation_id: Some("conv-abc-123".to_string()),
            search_mode: SearchMode::Auto,
            deferred: true,
            reasoning_effort: Some(ReasoningEffort::High),
            max_completion_tokens: Some(16384),
            temperature: Some(0.7),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: XAISettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.model, "grok-4.1-fast");
        assert_eq!(back.conversation_id.as_deref(), Some("conv-abc-123"));
        assert_eq!(back.search_mode, SearchMode::Auto);
        assert!(back.deferred);
        assert_eq!(back.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(back.max_completion_tokens, Some(16384));
        assert_eq!(back.temperature, Some(0.7));
    }

    #[test]
    fn test_settings_with_missing_optionals() {
        let json = r#"{
            "model": "grok-4.3",
            "conversation_id": null,
            "search_mode": "off",
            "deferred": false,
            "reasoning_effort": null,
            "max_completion_tokens": null,
            "temperature": null
        }"#;
        let s: XAISettings = serde_json::from_str(json).unwrap();
        assert_eq!(s.model, "grok-4.3");
        assert!(s.conversation_id.is_none());
        assert!(!s.deferred);
    }
}
