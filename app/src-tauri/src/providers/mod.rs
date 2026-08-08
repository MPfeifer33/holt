//! Provider abstraction layer.
//!
//! Routes to the correct provider implementation based on connection config
//! and protocol. Provides `build_provider()` for agent creation and
//! `send_provider_completion()` with inline think middleware for the engine.

pub mod anthropic;
pub mod gemini;
pub mod openai;
pub mod openai_responses;
pub mod types;
pub mod xai;

use std::sync::Arc;

use crate::runtime::agent::AnthropicSettings;
use crate::runtime::connection::{AgentProtocol, ConnectionConfig};
use crate::runtime::openai_responses_settings::OpenAIResponsesSettings;
use crate::runtime::xai_settings::XAISettings;

use self::anthropic::AnthropicProvider;
use self::gemini::GeminiProvider;
use self::openai::OpenAIProvider;
use self::openai_responses::OpenAIResponsesProvider;
use self::types::*;
use self::xai::XAIProvider;

// =============================================================================
// Provider construction
// =============================================================================

/// Build a provider for the given connection config and protocol.
///
/// Returns `Ok(None)` for SDK/MCP agents (they bypass the provider layer).
/// Returns `Ok(Some(provider))` for HTTP-based protocols.
/// Returns `Err(ProviderError)` if config is invalid.
///
/// Provider-specific settings are consulted only when the resolved endpoint
/// routes to their respective provider. `openai_responses_settings` is used
/// for `api.openai.com`, `xai_settings` for `api.x.ai`, etc. Unused settings
/// are ignored.
pub fn build_provider(
    connection: &ConnectionConfig,
    protocol: &AgentProtocol,
    api_key: Option<&str>,
    anthropic_settings: Option<&AnthropicSettings>,
    openai_responses_settings: Option<&OpenAIResponsesSettings>,
    xai_settings: Option<&XAISettings>,
) -> Result<Option<Arc<dyn CompletionProvider>>, ProviderError> {
    match protocol {
        AgentProtocol::AgentSdk | AgentProtocol::Mcp => Ok(None),
        AgentProtocol::Codex => Err(ProviderError::InvalidConfig(
            "Codex lane is not wired to the provider layer yet".to_string(),
        )),
        AgentProtocol::AnthropicMessages => {
            let settings =
                anthropic_settings.ok_or(ProviderError::MissingField("anthropic_settings"))?;
            let token = api_key.ok_or(ProviderError::MissingField("bearer_token"))?;
            Ok(Some(Arc::new(AnthropicProvider::new(
                token,
                &settings.model,
                settings.thinking_enabled,
                settings.thinking_budget,
            ))))
        }
        AgentProtocol::ChatCompletions => {
            let (endpoint, model) = extract_endpoint_and_model(connection)?;
            let provider = detect_and_build(
                &endpoint,
                &model,
                api_key,
                openai_responses_settings,
                xai_settings,
            )?;
            Ok(Some(provider))
        }
    }
}

/// Auto-detect the correct provider from endpoint URL and model name,
/// then construct it.
///
/// Routing rules (first match wins):
/// 1. `generativelanguage.googleapis.com` → `GeminiProvider`
/// 2. `api.x.ai` → `XAIProvider` (all Grok models, not just multi-agent)
/// 3. `api.openai.com` → `OpenAIResponsesProvider` (the Responses API path)
/// 4. anything else → `OpenAIProvider` (Chat Completions for OpenAI-compat
///    servers like Ollama, llama.cpp, LM Studio, Groq, Together, etc.)
///
/// When the endpoint resolves to a specific provider, the corresponding
/// settings struct is used if provided, otherwise a default is synthesized
/// using the supplied `model` name.
pub fn detect_and_build(
    endpoint: &str,
    model: &str,
    api_key: Option<&str>,
    openai_responses_settings: Option<&OpenAIResponsesSettings>,
    xai_settings: Option<&XAISettings>,
) -> Result<Arc<dyn CompletionProvider>, ProviderError> {
    if endpoint.contains("generativelanguage.googleapis.com") {
        Ok(Arc::new(GeminiProvider::new(endpoint, api_key, model)))
    } else if endpoint.contains("api.x.ai") {
        // All xAI models route through XAIProvider. The provider handles
        // both multi-agent (Responses API) and standard (Chat Completions)
        // modes internally based on model name.
        let settings = match xai_settings {
            Some(s) => s.clone(),
            None => XAISettings {
                model: model.to_string(),
                ..Default::default()
            },
        };
        Ok(Arc::new(XAIProvider::new(
            endpoint, api_key, model, settings,
        )))
    } else if endpoint.contains("api.openai.com") {
        let settings = match openai_responses_settings {
            Some(s) => s.clone(),
            None => OpenAIResponsesSettings {
                model: model.to_string(),
                ..Default::default()
            },
        };
        Ok(Arc::new(OpenAIResponsesProvider::new(
            endpoint, api_key, settings,
        )))
    } else {
        Ok(Arc::new(OpenAIProvider::new(endpoint, api_key, model)))
    }
}

/// Extract endpoint URL and model name from a ConnectionConfig.
///
/// Only works for Api and Local configs. Returns Err for MCP/OAuth/ClaudeCode
/// (those use different construction paths).
pub fn extract_endpoint_and_model(
    connection: &ConnectionConfig,
) -> Result<(String, String), ProviderError> {
    match connection {
        ConnectionConfig::Api {
            endpoint, model, ..
        } => Ok((endpoint.clone(), model.clone())),
        ConnectionConfig::Local { host, port, model } => {
            let endpoint = format!("http://{}:{}/v1", host, port);
            let model = model.clone().unwrap_or_else(|| "default".to_string());
            Ok((endpoint, model))
        }
        ConnectionConfig::OAuth { model, .. } => {
            // OAuth uses ANTHROPIC_API_URL, not a user-supplied endpoint
            Ok((
                self::anthropic::ANTHROPIC_API_URL.to_string(),
                model.clone(),
            ))
        }
        _ => Err(ProviderError::UnsupportedConnection(format!(
            "{:?}",
            connection
        ))),
    }
}

// =============================================================================
// Think middleware + unified send
// =============================================================================

/// Send a completion through a provider with inline `<think>`/`</think>` detection.
///
/// Wraps the provider's `send_completion()` with a callback that converts
/// `<think>` blocks into `StreamEventType::Thinking` events. This handles
/// local models that emit thinking tokens as plain text.
pub async fn send_provider_completion(
    provider: &dyn CompletionProvider,
    client: &reqwest::Client,
    messages: &[serde_json::Value],
    tools: Option<&[serde_json::Value]>,
    max_tokens: u32,
    on_token: &(dyn Fn(StreamEvent) + Send + Sync),
    agent_id: &str,
) -> Result<AssembledResponse, StreamError> {
    // Wrap callback to detect <think> blocks and emit Thinking events
    let in_think = std::sync::Mutex::new(false);
    let think_aware_callback = |event: StreamEvent| {
        if event.event_type == StreamEventType::Token {
            if let Some(text) = event.data.get("text").and_then(|v| v.as_str()) {
                let mut thinking = in_think.lock().unwrap_or_else(|e| e.into_inner());

                // Check for think tag transitions
                if text.contains("<think>") {
                    *thinking = true;
                    // Emit portion before <think> as Token, rest as Thinking
                    if let Some(idx) = text.find("<think>") {
                        let before = &text[..idx];
                        let after = &text[idx + 7..]; // skip "<think>"
                        if !before.is_empty() {
                            on_token(StreamEvent {
                                agent_id: event.agent_id.clone(),
                                event_type: StreamEventType::Token,
                                data: serde_json::json!({"text": before}),
                            });
                        }
                        if !after.is_empty() {
                            on_token(StreamEvent {
                                agent_id: event.agent_id.clone(),
                                event_type: StreamEventType::Thinking,
                                data: serde_json::json!({"text": after}),
                            });
                        }
                        return;
                    }
                }

                if text.contains("</think>") {
                    *thinking = false;
                    if let Some(idx) = text.find("</think>") {
                        let before = &text[..idx];
                        let after = &text[idx + 8..]; // skip "</think>"
                        if !before.is_empty() {
                            on_token(StreamEvent {
                                agent_id: event.agent_id.clone(),
                                event_type: StreamEventType::Thinking,
                                data: serde_json::json!({"text": before}),
                            });
                        }
                        if !after.is_empty() {
                            on_token(StreamEvent {
                                agent_id: event.agent_id.clone(),
                                event_type: StreamEventType::Token,
                                data: serde_json::json!({"text": after}),
                            });
                        }
                        return;
                    }
                }

                if *thinking {
                    on_token(StreamEvent {
                        agent_id: event.agent_id,
                        event_type: StreamEventType::Thinking,
                        data: event.data,
                    });
                    return;
                }
            }
        }
        on_token(event);
    };

    provider
        .send_completion(
            client,
            messages,
            tools,
            max_tokens,
            &think_aware_callback,
            agent_id,
        )
        .await
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::connection::*;
    use std::collections::HashMap;

    #[test]
    fn test_build_provider_sdk_returns_none() {
        let conn = ConnectionConfig::AgentSdk;
        let result = build_provider(&conn, &AgentProtocol::AgentSdk, None, None, None, None);
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_build_provider_mcp_returns_none() {
        let conn = ConnectionConfig::McpStdio {
            command: "test".into(),
            args: vec![],
            env: HashMap::new(),
        };
        let result = build_provider(&conn, &AgentProtocol::Mcp, None, None, None, None);
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_build_provider_xai_endpoint_routes_to_xai() {
        // All xAI endpoints now route to the dedicated XAIProvider,
        // not the generic OpenAI fallback. This gives access to xAI-specific
        // features like cache affinity, search, and deferred completions.
        let conn = ConnectionConfig::Api {
            endpoint: "https://api.x.ai/v1".into(),
            api_key_ref: "key".into(),
            model: "grok-4".into(),
        };
        let result = build_provider(
            &conn,
            &AgentProtocol::ChatCompletions,
            Some("xai-test"),
            None,
            None,
            None,
        );
        let provider = result.unwrap().unwrap();
        assert_eq!(provider.name(), "xAI");
    }

    #[test]
    fn test_build_provider_compat_endpoint_returns_openai_compat() {
        // Non-cloud OpenAI-compatible endpoints (Together, Groq, etc.)
        // still fall through to the generic OpenAI provider.
        let conn = ConnectionConfig::Api {
            endpoint: "https://api.together.xyz/v1".into(),
            api_key_ref: "key".into(),
            model: "llama3".into(),
        };
        let result = build_provider(
            &conn,
            &AgentProtocol::ChatCompletions,
            Some("test-key"),
            None,
            None,
            None,
        );
        let provider = result.unwrap().unwrap();
        assert_eq!(provider.name(), "OpenAI-compatible");
    }

    #[test]
    fn test_build_provider_openai_endpoint_routes_to_responses() {
        // api.openai.com with ChatCompletions protocol must build the
        // Responses provider, not the legacy Chat Completions provider.
        let conn = ConnectionConfig::Api {
            endpoint: "https://api.openai.com/v1".into(),
            api_key_ref: "key".into(),
            model: "gpt-5.4".into(),
        };
        let result = build_provider(
            &conn,
            &AgentProtocol::ChatCompletions,
            Some("sk-test"),
            None,
            None,
            None,
        );
        let provider = result.unwrap().unwrap();
        assert_eq!(provider.name(), "OpenAI Responses");
    }

    #[test]
    fn test_build_provider_anthropic_requires_settings() {
        let conn = ConnectionConfig::OAuth {
            provider: "anthropic".into(),
            credentials_path: "/tmp/test".into(),
            model: "claude-sonnet-4-20250514".into(),
        };
        let result = build_provider(
            &conn,
            &AgentProtocol::AnthropicMessages,
            Some("token"),
            None,
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_build_provider_anthropic_with_settings() {
        let conn = ConnectionConfig::OAuth {
            provider: "anthropic".into(),
            credentials_path: "/tmp/test".into(),
            model: "claude-sonnet-4-20250514".into(),
        };
        let settings = AnthropicSettings {
            model: "claude-sonnet-4-20250514".into(),
            thinking_enabled: true,
            thinking_budget: 10000,
            temperature: 1.0,
        };
        let result = build_provider(
            &conn,
            &AgentProtocol::AnthropicMessages,
            Some("token"),
            Some(&settings),
            None,
            None,
        );
        let provider = result.unwrap().unwrap();
        assert_eq!(provider.name(), "Anthropic Messages");
    }

    #[test]
    fn test_build_provider_codex_not_yet_wired() {
        let conn = ConnectionConfig::OAuth {
            provider: crate::runtime::codex::OPENAI_OAUTH_PROVIDER.into(),
            credentials_path: "/tmp/codex-auth.json".into(),
            model: crate::runtime::codex::default_model(),
        };
        let result = build_provider(&conn, &AgentProtocol::Codex, None, None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_detect_and_build_gemini() {
        let p = detect_and_build(
            "https://generativelanguage.googleapis.com/v1beta",
            "gemini-2.5-pro",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(p.name(), "Google Gemini");
    }

    #[test]
    fn test_detect_and_build_xai_multi_agent() {
        let p = detect_and_build(
            "https://api.x.ai/v1",
            "grok-4.20-multi-agent",
            Some("key"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(p.name(), "xAI");
    }

    #[test]
    fn test_detect_and_build_xai_standard_model() {
        // Standard Grok models (non-multi-agent) now route to XAIProvider
        // instead of falling through to the generic OpenAI adapter.
        for model in ["grok-4.3", "grok-4", "grok-4-fast", "grok-4.1-fast"] {
            let p =
                detect_and_build("https://api.x.ai/v1", model, Some("key"), None, None).unwrap();
            assert_eq!(p.name(), "xAI", "model {model} should route to xAI");
        }
    }

    #[test]
    fn test_detect_and_build_openai_responses() {
        let p = detect_and_build(
            "https://api.openai.com/v1",
            "gpt-5.4",
            Some("sk-test"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(p.name(), "OpenAI Responses");
    }

    #[test]
    fn test_detect_and_build_openai_responses_synthesizes_default_settings() {
        let p = detect_and_build(
            "https://api.openai.com/v1",
            "gpt-5.4-mini",
            Some("sk-test"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(p.name(), "OpenAI Responses");
    }

    #[test]
    fn test_detect_and_build_openai_compat_fallback() {
        // localhost endpoints (Ollama, llama.cpp, LM Studio, etc.) and any
        // other endpoint that isn't api.openai.com / Gemini / xAI
        // must continue to use the legacy Chat Completions OpenAIProvider.
        let p =
            detect_and_build("http://localhost:8080/v1", "qwen3-coder", None, None, None).unwrap();
        assert_eq!(p.name(), "OpenAI-compatible");
    }

    #[test]
    fn test_extract_endpoint_api() {
        let conn = ConnectionConfig::Api {
            endpoint: "https://api.openai.com/v1".into(),
            api_key_ref: "key".into(),
            model: "gpt-4".into(),
        };
        let (ep, model) = extract_endpoint_and_model(&conn).unwrap();
        assert_eq!(ep, "https://api.openai.com/v1");
        assert_eq!(model, "gpt-4");
    }

    #[test]
    fn test_extract_endpoint_local() {
        let conn = ConnectionConfig::Local {
            host: "localhost".into(),
            port: 8080,
            model: Some("qwen3".into()),
        };
        let (ep, model) = extract_endpoint_and_model(&conn).unwrap();
        assert_eq!(ep, "http://localhost:8080/v1");
        assert_eq!(model, "qwen3");
    }

    #[test]
    fn test_extract_endpoint_unsupported() {
        let conn = ConnectionConfig::AgentSdk;
        assert!(extract_endpoint_and_model(&conn).is_err());
    }
}
