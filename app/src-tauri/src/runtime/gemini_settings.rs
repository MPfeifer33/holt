// app/src-tauri/src/runtime/gemini_settings.rs
//
// Runtime settings for agents using Google's Gemini models.
//
// Parallel to `runtime::openai_responses_settings::OpenAIResponsesSettings` —
// each agent can have its own Gemini configuration stored on its `AgentSlot`.
//
// Gemini models have several unique capabilities:
//
// - **Explicit caching**: Server-side cache for large contexts (tools, system
//   prompt, persona). Reduces cost by ~75% on cached tokens. Requires minimum
//   32K tokens of cacheable content.
// - **Thinking mode**: Gemini 2.5 Pro/Flash support a thinking mode with
//   configurable token budgets, similar to Claude's extended thinking.
// - **Google Search grounding**: Native web search integration that includes
//   grounding citations in the response.
// - **top_k sampling**: Unique to Gemini — controls the number of top tokens
//   considered during sampling.
//
use serde::{Deserialize, Serialize};

/// Per-agent runtime configuration for Google Gemini models.
///
/// Stored on `AgentSlot` as an `Option` so agents that don't use Gemini have
/// zero footprint. Populated when an agent's connection endpoint resolves to
/// `generativelanguage.googleapis.com`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiSettings {
    /// Model name (e.g., "gemini-2.5-pro", "gemini-2.5-flash",
    /// "gemini-2.5-flash-lite").
    pub model: String,

    /// Sampling temperature (0.0 – 2.0). `None` means use model default.
    pub temperature: Option<f32>,

    /// Top-p nucleus sampling. `None` means use model default.
    pub top_p: Option<f32>,

    /// Top-k sampling (unique to Gemini). Controls how many top tokens are
    /// considered during sampling. `None` means use model default (typically 40).
    pub top_k: Option<u32>,

    /// Maximum output tokens. `None` means use model default.
    pub max_output_tokens: Option<u32>,

    /// Whether to use Gemini's explicit caching for large contexts.
    /// When enabled and the context exceeds ~32K tokens, the provider will
    /// create a server-side cache for tools, system prompt, and persona content.
    pub explicit_cache_enabled: bool,

    /// TTL for the explicit cache in seconds. Default 3600 (1 hour).
    /// Gemini charges for cache storage time, so this balances cost vs. reuse.
    pub cache_ttl_seconds: u32,

    /// Whether to enable thinking mode (Gemini 2.5+ models).
    /// When enabled, the model performs internal reasoning before responding,
    /// similar to Claude's extended thinking.
    pub thinking_enabled: bool,

    /// Token budget for thinking mode. Only used when `thinking_enabled` is true.
    /// `None` means use model default.
    pub thinking_budget: Option<u32>,

    /// Whether to enable Google Search grounding.
    /// When enabled, Gemini can perform web searches and include grounding
    /// citations in its response. Note: charged at $14/1K queries after 5K
    /// free queries per month.
    pub grounding_enabled: bool,
}

impl GeminiSettings {
    /// Returns true if this model supports thinking mode.
    /// Currently Gemini 2.5 Pro and 2.5 Flash support thinking.
    pub fn supports_thinking(&self) -> bool {
        self.model.contains("2.5")
    }

    /// Returns true if sampling parameters should be omitted.
    /// When thinking is enabled on supported models, some sampling params
    /// may be restricted.
    pub fn allows_sampling_params(&self) -> bool {
        // Gemini doesn't restrict sampling params when thinking is enabled
        // (unlike OpenAI/Anthropic). All sampling params are always valid.
        true
    }
}

impl Default for GeminiSettings {
    /// Defaults:
    /// - `model`: "gemini-2.5-flash" (balanced speed/quality)
    /// - `temperature`, `top_p`, `top_k`, `max_output_tokens`: `None` (model defaults)
    /// - `explicit_cache_enabled`: `false` (opt-in, has cost implications)
    /// - `cache_ttl_seconds`: 3600 (1 hour)
    /// - `thinking_enabled`: `false` (opt-in)
    /// - `thinking_budget`: `None` (model default)
    /// - `grounding_enabled`: `false` (opt-in, has cost implications)
    fn default() -> Self {
        Self {
            model: "gemini-2.5-flash".to_string(),
            temperature: None,
            top_p: None,
            top_k: None,
            max_output_tokens: None,
            explicit_cache_enabled: false,
            cache_ttl_seconds: 3600,
            thinking_enabled: false,
            thinking_budget: None,
            grounding_enabled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let s = GeminiSettings::default();
        assert_eq!(s.model, "gemini-2.5-flash");
        assert!(s.temperature.is_none());
        assert!(s.top_p.is_none());
        assert!(s.top_k.is_none());
        assert!(s.max_output_tokens.is_none());
        assert!(!s.explicit_cache_enabled);
        assert_eq!(s.cache_ttl_seconds, 3600);
        assert!(!s.thinking_enabled);
        assert!(s.thinking_budget.is_none());
        assert!(!s.grounding_enabled);
    }

    #[test]
    fn test_supports_thinking() {
        let mut s = GeminiSettings::default();
        s.model = "gemini-2.5-pro".into();
        assert!(s.supports_thinking());

        s.model = "gemini-2.5-flash".into();
        assert!(s.supports_thinking());

        s.model = "gemini-2.5-flash-lite".into();
        assert!(s.supports_thinking());

        s.model = "gemini-2.0-flash".into();
        assert!(!s.supports_thinking());
    }

    #[test]
    fn test_full_settings_serde_roundtrip() {
        let s = GeminiSettings {
            model: "gemini-2.5-pro".to_string(),
            temperature: Some(0.8),
            top_p: Some(0.95),
            top_k: Some(40),
            max_output_tokens: Some(8192),
            explicit_cache_enabled: true,
            cache_ttl_seconds: 7200,
            thinking_enabled: true,
            thinking_budget: Some(4096),
            grounding_enabled: true,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: GeminiSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.model, "gemini-2.5-pro");
        assert_eq!(back.temperature, Some(0.8));
        assert_eq!(back.top_p, Some(0.95));
        assert_eq!(back.top_k, Some(40));
        assert_eq!(back.max_output_tokens, Some(8192));
        assert!(back.explicit_cache_enabled);
        assert_eq!(back.cache_ttl_seconds, 7200);
        assert!(back.thinking_enabled);
        assert_eq!(back.thinking_budget, Some(4096));
        assert!(back.grounding_enabled);
    }

    #[test]
    fn test_settings_with_missing_optionals() {
        let json = r#"{
            "model": "gemini-2.5-flash",
            "temperature": null,
            "top_p": null,
            "top_k": null,
            "max_output_tokens": null,
            "explicit_cache_enabled": false,
            "cache_ttl_seconds": 3600,
            "thinking_enabled": false,
            "thinking_budget": null,
            "grounding_enabled": false
        }"#;
        let s: GeminiSettings = serde_json::from_str(json).unwrap();
        assert_eq!(s.model, "gemini-2.5-flash");
        assert!(!s.explicit_cache_enabled);
        assert!(!s.thinking_enabled);
    }

    #[test]
    fn test_allows_sampling_params_always_true() {
        let mut s = GeminiSettings::default();
        s.thinking_enabled = true;
        assert!(s.allows_sampling_params());
    }
}
