// app/src-tauri/src/runtime/local_model_settings.rs
//
// Runtime settings for agents using local models (Ollama, LM Studio,
// llama.cpp, or any OpenAI-compatible local endpoint).
//
// Local models are unique because:
//
// - **Capabilities vary wildly.** Some support tool calling, some don't. Some
//   support vision, some don't. Context windows range from 2K to 128K+.
//   We can't assume anything — detect or let the user override.
// - **No cost.** Local inference is free (electricity aside), which changes
//   the calculus for agent usage.
// - **Think tag detection.** Many local models emit `<think>` blocks as plain
//   text. The think middleware in `send_provider_completion` already handles
//   this, but it should be togglable per-agent.
// - **Provider-specific APIs.** Ollama has `/api/show` for model info,
//   llama.cpp has `/props`, LM Studio has its own model listing. We need to
//   know which type we're talking to for auto-detection.
//
use serde::{Deserialize, Serialize};

/// Serde default function: true. Used for `strict_system_position` so agents
/// created before the field existed deserialize safely (strict = on).
fn default_strict_true() -> bool {
    true
}

/// Per-agent runtime configuration for local models.
///
/// Stored on `AgentSlot` as an `Option` so agents that don't use local models
/// have zero footprint. Populated when an agent's connection resolves to a
/// local endpoint (localhost, 127.0.0.1, or other non-cloud hosts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModelSettings {
    /// Model name (e.g., "qwen2.5:14b", "llama3.1:8b", "deepseek-r1:32b").
    pub model: String,

    /// Which local model server is running.
    pub provider_type: LocalProviderType,

    /// Full endpoint URL (e.g., "http://localhost:11434/v1").
    pub endpoint: String,

    /// Context window size in tokens. Auto-detected from the provider's model
    /// info API, or manually overridden by the user.
    pub context_window: u32,

    /// Whether the model supports tool/function calling. Auto-detected or
    /// manually overridden. When false, the provider omits tools from requests.
    pub supports_tools: bool,

    /// Whether the model supports vision/image input. Auto-detected or
    /// manually overridden.
    pub supports_vision: bool,

    /// Sampling temperature (0.0 – 2.0). `None` means use model default.
    pub temperature: Option<f32>,

    /// Top-p nucleus sampling. `None` means use model default.
    pub top_p: Option<f32>,

    /// GPU layers for llama.cpp (`n_gpu_layers`). `None` means use server
    /// default. Only relevant for llama.cpp provider type.
    pub gpu_layers: Option<u32>,

    /// Whether to detect `<think>`/`</think>` blocks in the model output and
    /// convert them to `StreamEventType::Thinking` events. Many local models
    /// (especially DeepSeek-R1 derivatives) emit thinking as plain text.
    pub think_tag_detection: bool,

    /// If true, rewrite mid-conversation system messages to `role: user` with
    /// a `[SYSTEM CONTEXT]` wrapper before sending to the inference server.
    ///
    /// Required for models whose Jinja chat templates enforce that system
    /// messages must be at position 0 (e.g., Qwen-family, Ornith). Without
    /// this, compaction summaries, session memory, circuit breakers, A2A
    /// messages, and other mid-conversation system injections trigger HTTP 500
    /// from the inference server.
    ///
    /// Defaults to `true` for `LlamaCpp` (most deployments use the model's
    /// embedded template, which is strict). Harmless when enabled for models
    /// that don't enforce ordering — the wrapper is just text.
    ///
    /// Serde default is `true` so agents created before this field existed
    /// (whose persisted config lacks the key) get safe behavior automatically.
    #[serde(default = "default_strict_true")]
    pub strict_system_position: bool,
}

/// Type of local model server.
///
/// Each provider type has different auto-detection APIs and capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalProviderType {
    /// Ollama — serves models on port 11434 by default. Has `/api/show` for
    /// model info (context window, capabilities).
    Ollama,
    /// LM Studio — serves models on port 1234 by default. OpenAI-compatible
    /// API with model listing.
    LmStudio,
    /// llama.cpp server — serves on port 8080 by default. Has `/props` for
    /// server info and model metadata.
    LlamaCpp,
    /// Any OpenAI-compatible endpoint that doesn't match the above.
    /// No auto-detection, user must specify capabilities manually.
    Custom,
}

impl LocalProviderType {
    pub fn as_str(&self) -> &'static str {
        match self {
            LocalProviderType::Ollama => "ollama",
            LocalProviderType::LmStudio => "lmstudio",
            LocalProviderType::LlamaCpp => "llamacpp",
            LocalProviderType::Custom => "custom",
        }
    }

    /// Default port for this provider type.
    pub fn default_port(&self) -> u16 {
        match self {
            LocalProviderType::Ollama => 11434,
            LocalProviderType::LmStudio => 1234,
            LocalProviderType::LlamaCpp => 8080,
            LocalProviderType::Custom => 8080,
        }
    }
}

impl Default for LocalProviderType {
    fn default() -> Self {
        Self::Ollama
    }
}

/// Detect provider type from the endpoint URL based on port conventions.
///
/// - Port 11434 → Ollama
/// - Port 1234 → LM Studio
/// - Port 8080 → llama.cpp
/// - Anything else → Custom
pub fn detect_provider_type(endpoint: &str) -> LocalProviderType {
    if endpoint.contains(":11434") {
        LocalProviderType::Ollama
    } else if endpoint.contains(":1234") {
        LocalProviderType::LmStudio
    } else if endpoint.contains(":8080") {
        LocalProviderType::LlamaCpp
    } else {
        LocalProviderType::Custom
    }
}

impl Default for LocalModelSettings {
    /// Defaults:
    /// - `model`: "default" (placeholder until auto-detected)
    /// - `provider_type`: `Ollama` (most common local provider)
    /// - `endpoint`: "http://localhost:11434/v1"
    /// - `context_window`: 8192 (conservative default)
    /// - `supports_tools`: true (most modern local models support tool calling)
    /// - `supports_vision`: false (don't assume vision support)
    /// - `temperature`, `top_p`: `None` (model defaults)
    /// - `gpu_layers`: `None` (server default)
    /// - `think_tag_detection`: true (most local models benefit from this)
    fn default() -> Self {
        Self {
            model: "default".to_string(),
            provider_type: LocalProviderType::default(),
            endpoint: "http://localhost:11434/v1".to_string(),
            context_window: 262144,
            supports_tools: true,
            supports_vision: false,
            temperature: None,
            top_p: None,
            gpu_layers: None,
            think_tag_detection: true,
            strict_system_position: false,
        }
    }
}

impl LocalModelSettings {
    /// Returns whether strict system position should be enabled based on
    /// provider type. Called during agent creation to set the default — users
    /// can override afterward.
    pub fn default_strict_system_position(provider_type: LocalProviderType) -> bool {
        matches!(provider_type, LocalProviderType::LlamaCpp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let s = LocalModelSettings::default();
        assert_eq!(s.model, "default");
        assert_eq!(s.provider_type, LocalProviderType::Ollama);
        assert_eq!(s.endpoint, "http://localhost:11434/v1");
        assert_eq!(s.context_window, 262144);
        assert!(s.supports_tools);
        assert!(!s.supports_vision);
        assert!(s.temperature.is_none());
        assert!(s.top_p.is_none());
        assert!(s.gpu_layers.is_none());
        assert!(s.think_tag_detection);
        assert!(!s.strict_system_position); // default false (Ollama)
    }

    #[test]
    fn test_strict_system_position_defaults_by_provider() {
        assert!(
            LocalModelSettings::default_strict_system_position(LocalProviderType::LlamaCpp),
            "LlamaCpp should default strict_system_position to true"
        );
        assert!(
            !LocalModelSettings::default_strict_system_position(LocalProviderType::Ollama),
            "Ollama should default strict_system_position to false"
        );
        assert!(
            !LocalModelSettings::default_strict_system_position(LocalProviderType::LmStudio),
            "LmStudio should default strict_system_position to false"
        );
        assert!(
            !LocalModelSettings::default_strict_system_position(LocalProviderType::Custom),
            "Custom should default strict_system_position to false"
        );
    }

    #[test]
    fn test_provider_type_serde_roundtrip() {
        for pt in [
            LocalProviderType::Ollama,
            LocalProviderType::LmStudio,
            LocalProviderType::LlamaCpp,
            LocalProviderType::Custom,
        ] {
            let json = serde_json::to_string(&pt).unwrap();
            let back: LocalProviderType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, pt, "round-trip failed for {:?}", pt);
        }
    }

    #[test]
    fn test_provider_type_as_str() {
        assert_eq!(LocalProviderType::Ollama.as_str(), "ollama");
        assert_eq!(LocalProviderType::LmStudio.as_str(), "lmstudio");
        assert_eq!(LocalProviderType::LlamaCpp.as_str(), "llamacpp");
        assert_eq!(LocalProviderType::Custom.as_str(), "custom");
    }

    #[test]
    fn test_default_ports() {
        assert_eq!(LocalProviderType::Ollama.default_port(), 11434);
        assert_eq!(LocalProviderType::LmStudio.default_port(), 1234);
        assert_eq!(LocalProviderType::LlamaCpp.default_port(), 8080);
        assert_eq!(LocalProviderType::Custom.default_port(), 8080);
    }

    #[test]
    fn test_detect_provider_type_from_endpoint() {
        assert_eq!(
            detect_provider_type("http://localhost:11434/v1"),
            LocalProviderType::Ollama
        );
        assert_eq!(
            detect_provider_type("http://192.168.1.100:11434/v1"),
            LocalProviderType::Ollama
        );
        assert_eq!(
            detect_provider_type("http://localhost:1234/v1"),
            LocalProviderType::LmStudio
        );
        assert_eq!(
            detect_provider_type("http://localhost:8080/v1"),
            LocalProviderType::LlamaCpp
        );
        assert_eq!(
            detect_provider_type("http://localhost:5000/v1"),
            LocalProviderType::Custom
        );
    }

    #[test]
    fn test_full_settings_serde_roundtrip() {
        let s = LocalModelSettings {
            model: "qwen2.5:14b".to_string(),
            provider_type: LocalProviderType::Ollama,
            endpoint: "http://localhost:11434/v1".to_string(),
            context_window: 32768,
            supports_tools: true,
            supports_vision: false,
            temperature: Some(0.7),
            top_p: Some(0.9),
            gpu_layers: None,
            think_tag_detection: true,
            strict_system_position: false,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: LocalModelSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.model, "qwen2.5:14b");
        assert_eq!(back.provider_type, LocalProviderType::Ollama);
        assert_eq!(back.context_window, 32768);
        assert!(back.supports_tools);
        assert!(!back.supports_vision);
        assert_eq!(back.temperature, Some(0.7));
        assert!(back.think_tag_detection);
    }

    #[test]
    fn test_llamacpp_with_gpu_layers() {
        let s = LocalModelSettings {
            model: "llama3.1:8b".to_string(),
            provider_type: LocalProviderType::LlamaCpp,
            endpoint: "http://localhost:8080/v1".to_string(),
            context_window: 131072,
            supports_tools: true,
            supports_vision: true,
            temperature: None,
            top_p: None,
            gpu_layers: Some(35),
            think_tag_detection: false,
            strict_system_position: true,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: LocalModelSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.gpu_layers, Some(35));
        assert_eq!(back.provider_type, LocalProviderType::LlamaCpp);
        assert!(!back.think_tag_detection);
    }

    #[test]
    fn test_missing_strict_system_position_defaults_true() {
        // Agents created before the field existed won't have it in their
        // persisted config. Serde must default to true (safe) so the
        // system message rewrite fires automatically.
        let json = r#"{
            "model": "ornith-1.0:35b",
            "provider_type": "llamacpp",
            "endpoint": "http://localhost:8080/v1",
            "context_window": 262144,
            "supports_tools": true,
            "supports_vision": false,
            "temperature": 0.6,
            "top_p": null,
            "gpu_layers": null,
            "think_tag_detection": true
        }"#;
        let settings: LocalModelSettings = serde_json::from_str(json).unwrap();
        assert!(
            settings.strict_system_position,
            "missing strict_system_position should default to true via serde"
        );
    }
}
