// app/src-tauri/src/runtime/openai_responses_settings.rs
//
// Runtime settings for agents using OpenAI's Responses API (GPT-5 family).
//
// Parallel to `runtime::agent::AnthropicSettings` — each agent can have its own
// OpenAI Responses configuration stored on its `AgentSlot`. The settings are
// persisted via the `persistence::session::AgentSession` mirror struct so user
// preferences (reasoning effort, verbosity, chosen GPT-5 variant) survive
// restarts.
//
// The Responses API is OpenAI's agent-oriented successor to Chat Completions.
// It supports reasoning-aware tool calling via GPT-5.4 and the o-series models,
// correlates tool_calls to tool outputs by `call_id` rather than position in
// the input array, and exposes `reasoning.effort` + `text.verbosity` +
// `max_output_tokens` as first-class request fields.
//
// Conditional parameter gating: when `reasoning_effort != None`, the Responses
// API rejects `temperature`, `top_p`, and `logprobs` fields with a 400 error.
// The `OpenAIResponsesProvider`'s request builder checks
// `ReasoningEffort::allows_sampling_params()` and omits those fields
// accordingly. This is a load-bearing invariant — see the test
// `test_allows_sampling_params` below.
//
use serde::{Deserialize, Serialize};

/// Per-agent runtime configuration for the OpenAI Responses API.
///
/// Stored on `AgentSlot` as an `Option` so agents that don't use the Responses
/// protocol have zero footprint. Populated when an agent's connection endpoint
/// resolves to `api.openai.com` and the agent is created or configured.
///
/// Serialized via `persistence::session::AgentSession`'s mirror field with
/// `#[serde(default)]` for backward compatibility with pre-existing
/// `session.json` files that do not contain this field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIResponsesSettings {
    /// Model name (e.g., "gpt-5.4", "gpt-5.4-pro", "gpt-5.4-mini", "gpt-5.4-nano").
    pub model: String,

    /// Reasoning depth. Per OpenAI's documentation, `None` is the default for
    /// latency-friendly interactions. Increase to `Medium` or higher when you
    /// need the model to think before responding.
    ///
    /// IMPORTANT: when this is not `None`, the Responses API rejects requests
    /// that include `temperature`, `top_p`, or `logprobs`. The provider's
    /// request builder enforces this automatically via
    /// `allows_sampling_params()`.
    pub reasoning_effort: ReasoningEffort,

    /// Output verbosity. Controls output token count independently of reasoning
    /// depth. `Medium` is OpenAI's default.
    pub verbosity: Verbosity,

    /// Maximum number of output tokens. `None` means use the model default.
    ///
    /// Note: this is the Responses API's `max_output_tokens` field, NOT the
    /// Chat Completions `max_tokens` / `max_completion_tokens`.
    pub max_output_tokens: Option<u32>,

    /// Sampling temperature (0.0 – 2.0). Only sent to the API when
    /// `reasoning_effort == None` (the API rejects it otherwise).
    pub temperature: Option<f32>,

    /// Top-p nucleus sampling. Only sent when `reasoning_effort == None`.
    pub top_p: Option<f32>,
}

/// Reasoning effort levels supported by the GPT-5 family via the Responses API.
///
/// Serialized as lowercase strings so the Holt-side value ("none", "medium")
/// round-trips cleanly with `session.json` persistence. This enum does NOT
/// serialize to the async-openai `ReasoningEffort` enum directly — the
/// `OpenAIResponsesProvider` calls `map_effort()` to convert at request-build
/// time, keeping Holt's internal representation independent of the crate's
/// evolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// No reasoning. Lowest latency. OpenAI's documented default for GPT-5.2+.
    /// This is the only effort level that allows `temperature` / `top_p`.
    None,
    /// Minimal reasoning (GPT-5 / o-series "minimal" variant).
    Minimal,
    /// Low reasoning effort.
    Low,
    /// Medium reasoning effort. Recommended first step up from `None` for
    /// tasks that need a bit of thinking before responding.
    Medium,
    /// High reasoning effort. Deeper, slower thinking.
    High,
    /// Extra high reasoning effort (GPT-5.4+ only).
    Xhigh,
}

impl ReasoningEffort {
    /// Returns true if this effort level allows the Responses API to accept
    /// `temperature`, `top_p`, and `logprobs` in the request body.
    ///
    /// Only `None` allows them. All other effort levels cause the API to
    /// return a 400 "unsupported parameter" error if those fields are present.
    /// The provider's request builder calls this to decide whether to include
    /// those fields at all.
    pub fn allows_sampling_params(&self) -> bool {
        matches!(self, ReasoningEffort::None)
    }

    /// Return the lowercase string representation used by the Responses API
    /// wire format. Useful for logging and tracing.
    pub fn as_str(&self) -> &'static str {
        match self {
            ReasoningEffort::None => "none",
            ReasoningEffort::Minimal => "minimal",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
            ReasoningEffort::Xhigh => "xhigh",
        }
    }
}

impl Default for ReasoningEffort {
    /// Default is `Medium`. OpenAI documents `None` as their default for chat
    /// use cases where latency dominates, but Holt runs these models in
    /// agentic contexts where the model needs to deliberate before acting:
    /// checking its own conversation history, reasoning about which tool to
    /// call, and avoiding repeating actions it has already completed.
    ///
    /// Without reasoning, GPT-5.4 has been observed exhibiting repetition /
    /// failure-to-stop on tool-call-heavy multi-turn flows (especially A2A
    /// coordination, where the model would re-send the same A2A message
    /// 8-10 times because it had no deliberation step to notice it had
    /// already sent it). The model itself self-identified the behavior as
    /// "model repetition / failure-to-stop" during internal harness testing.
    ///
    /// `Medium` gives meaningful deliberation without the latency hit of
    /// `High` / `Xhigh`. Users can override per-agent if they specifically
    /// need the latency-friendly path or temperature/top_p sampling — the
    /// Responses API only allows those parameters when `reasoning_effort
    /// == None`, which is no longer the default.
    fn default() -> Self {
        Self::Medium
    }
}

/// Output verbosity levels for the GPT-5 family via the Responses API.
///
/// Controls output token count. Independent of `ReasoningEffort` — the model
/// can reason deeply and respond concisely, or reason minimally and respond
/// verbosely. Default is `Medium`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    /// Concise answers. Favors shorter output, e.g., for SQL generation or
    /// terse code snippets.
    Low,
    /// Default. Balanced output length.
    Medium,
    /// Thorough explanations. Longer output, e.g., for code refactoring
    /// commentary or document summarization.
    High,
}

impl Verbosity {
    /// Return the lowercase string representation for the Responses API wire
    /// format.
    pub fn as_str(&self) -> &'static str {
        match self {
            Verbosity::Low => "low",
            Verbosity::Medium => "medium",
            Verbosity::High => "high",
        }
    }
}

impl Default for Verbosity {
    fn default() -> Self {
        Self::Medium
    }
}

impl Default for OpenAIResponsesSettings {
    /// Defaults:
    /// - `model`: "gpt-5.4" (flagship general-purpose model)
    /// - `reasoning_effort`: `Medium` — overrides OpenAI's `None` default
    ///   because Holt runs these models in agentic contexts where
    ///   deliberation matters more than latency. See
    ///   `ReasoningEffort::default()` for the full rationale.
    /// - `verbosity`: `Medium` (OpenAI's documented default — appropriate
    ///   for both chat and agent use cases)
    /// - `max_output_tokens`: `None` (model default)
    /// - `temperature`, `top_p`: `None` (model defaults; only sent when
    ///   `reasoning_effort == None`, which is no longer the default)
    fn default() -> Self {
        Self {
            model: "gpt-5.4".to_string(),
            reasoning_effort: ReasoningEffort::default(),
            verbosity: Verbosity::default(),
            max_output_tokens: None,
            temperature: None,
            top_p: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let s = OpenAIResponsesSettings::default();
        assert_eq!(s.model, "gpt-5.4");
        assert_eq!(s.reasoning_effort, ReasoningEffort::Medium);
        assert_eq!(s.verbosity, Verbosity::Medium);
        assert!(s.max_output_tokens.is_none());
        assert!(s.temperature.is_none());
        assert!(s.top_p.is_none());
    }

    #[test]
    fn test_reasoning_effort_serde_roundtrip_all_variants() {
        for effort in [
            ReasoningEffort::None,
            ReasoningEffort::Minimal,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::Xhigh,
        ] {
            let json = serde_json::to_string(&effort).unwrap();
            let back: ReasoningEffort = serde_json::from_str(&json).unwrap();
            assert_eq!(back, effort, "round-trip failed for {:?}", effort);
        }
    }

    #[test]
    fn test_reasoning_effort_serializes_as_lowercase() {
        // Wire format is lowercase strings — matches the Responses API.
        assert_eq!(
            serde_json::to_string(&ReasoningEffort::None).unwrap(),
            "\"none\""
        );
        assert_eq!(
            serde_json::to_string(&ReasoningEffort::Minimal).unwrap(),
            "\"minimal\""
        );
        assert_eq!(
            serde_json::to_string(&ReasoningEffort::Low).unwrap(),
            "\"low\""
        );
        assert_eq!(
            serde_json::to_string(&ReasoningEffort::Medium).unwrap(),
            "\"medium\""
        );
        assert_eq!(
            serde_json::to_string(&ReasoningEffort::High).unwrap(),
            "\"high\""
        );
        assert_eq!(
            serde_json::to_string(&ReasoningEffort::Xhigh).unwrap(),
            "\"xhigh\""
        );
    }

    #[test]
    fn test_reasoning_effort_as_str() {
        assert_eq!(ReasoningEffort::None.as_str(), "none");
        assert_eq!(ReasoningEffort::Minimal.as_str(), "minimal");
        assert_eq!(ReasoningEffort::Low.as_str(), "low");
        assert_eq!(ReasoningEffort::Medium.as_str(), "medium");
        assert_eq!(ReasoningEffort::High.as_str(), "high");
        assert_eq!(ReasoningEffort::Xhigh.as_str(), "xhigh");
    }

    #[test]
    fn test_allows_sampling_params() {
        // Load-bearing invariant: only None allows temperature/top_p/logprobs.
        // The provider's request builder depends on this.
        assert!(ReasoningEffort::None.allows_sampling_params());
        assert!(!ReasoningEffort::Minimal.allows_sampling_params());
        assert!(!ReasoningEffort::Low.allows_sampling_params());
        assert!(!ReasoningEffort::Medium.allows_sampling_params());
        assert!(!ReasoningEffort::High.allows_sampling_params());
        assert!(!ReasoningEffort::Xhigh.allows_sampling_params());
    }

    #[test]
    fn test_reasoning_effort_default_is_medium() {
        // Medium reasoning is Holt's chosen default for the agent use
        // case. OpenAI's own default for GPT-5.4 is `None` (chat-friendly,
        // low-latency), but Holt runs these models in tool-call-heavy
        // agentic contexts where the model needs to deliberate before
        // acting to avoid repetition / failure-to-stop loops. If this test
        // ever fails because someone changed the default, check the doc
        // comment on `ReasoningEffort::default()` for the rationale.
        assert_eq!(ReasoningEffort::default(), ReasoningEffort::Medium);
    }

    #[test]
    fn test_verbosity_serde_roundtrip() {
        for v in [Verbosity::Low, Verbosity::Medium, Verbosity::High] {
            let json = serde_json::to_string(&v).unwrap();
            let back: Verbosity = serde_json::from_str(&json).unwrap();
            assert_eq!(back, v);
        }
    }

    #[test]
    fn test_verbosity_as_str() {
        assert_eq!(Verbosity::Low.as_str(), "low");
        assert_eq!(Verbosity::Medium.as_str(), "medium");
        assert_eq!(Verbosity::High.as_str(), "high");
    }

    #[test]
    fn test_verbosity_default_is_medium() {
        assert_eq!(Verbosity::default(), Verbosity::Medium);
    }

    #[test]
    fn test_full_settings_serde_roundtrip() {
        let s = OpenAIResponsesSettings {
            model: "gpt-5.4-pro".to_string(),
            reasoning_effort: ReasoningEffort::High,
            verbosity: Verbosity::Low,
            max_output_tokens: Some(8192),
            temperature: Some(0.7),
            top_p: Some(0.95),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: OpenAIResponsesSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.model, "gpt-5.4-pro");
        assert_eq!(back.reasoning_effort, ReasoningEffort::High);
        assert_eq!(back.verbosity, Verbosity::Low);
        assert_eq!(back.max_output_tokens, Some(8192));
        assert_eq!(back.temperature, Some(0.7));
        assert_eq!(back.top_p, Some(0.95));
    }

    #[test]
    fn test_settings_serde_with_missing_optional_fields() {
        // Verify that a minimal JSON payload (required fields only) still
        // deserializes when the Option<_> fields are absent. This matches how
        // a freshly-synthesized default from `detect_and_build()` would look
        // if the agent hadn't yet saved any explicit values.
        let json = r#"{
            "model": "gpt-5.4",
            "reasoning_effort": "none",
            "verbosity": "medium",
            "max_output_tokens": null,
            "temperature": null,
            "top_p": null
        }"#;
        let s: OpenAIResponsesSettings = serde_json::from_str(json).unwrap();
        assert_eq!(s.model, "gpt-5.4");
        assert_eq!(s.reasoning_effort, ReasoningEffort::None);
        assert_eq!(s.verbosity, Verbosity::Medium);
        assert!(s.max_output_tokens.is_none());
    }
}
