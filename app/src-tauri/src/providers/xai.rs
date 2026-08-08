//! xAI provider for all Grok models.
//!
//! Routes to the correct API surface internally:
//! - **Multi-agent models** (`*-multi-agent`): Uses `/v1/responses` (xAI Responses API)
//!   with SSE events like `response.output_text.delta` and `response.completed`.
//!   Does not support client-side tools.
//! - **Standard models** (grok-4.3, grok-4, grok-4.1-fast, etc.): Uses
//!   `/v1/chat/completions` (OpenAI-compatible Chat Completions API) with full
//!   tool calling, reasoning_content streaming, and xAI-specific features.
//!
//! xAI-specific features exposed:
//! - `x-grok-conv-id` header for cache affinity (75-90% discount on input tokens)
//! - `search_parameters` for server-side web/X search
//! - `deferred` mode for fire-and-forget long tasks
//! - Always-on reasoning detection (grok-4.3, grok-4)
//! - `cost_in_usd_ticks` parsing from usage responses

use async_trait::async_trait;
use futures::StreamExt;
use std::collections::HashMap;

use crate::providers::types::*;
use crate::runtime::agent::ToolCallRecord;
use crate::runtime::xai_settings::{is_always_reasoning_model, XAISettings};

// =============================================================================
// Chat Completions deserialization structs (same format as OpenAI)
// =============================================================================

#[derive(Debug, serde::Deserialize)]
struct ChatCompletionChunk {
    choices: Option<Vec<ChunkChoice>>,
    usage: Option<UsageInfo>,
}

#[derive(Debug, serde::Deserialize)]
struct ChunkChoice {
    delta: Option<ChunkDelta>,
    finish_reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ChunkDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<ChunkToolCall>>,
}

#[derive(Debug, serde::Deserialize)]
struct ChunkToolCall {
    index: Option<usize>,
    id: Option<String>,
    function: Option<ChunkFunction>,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    call_type: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ChunkFunction {
    name: Option<String>,
    arguments: Option<String>,
}

struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

// =============================================================================
// XAIProvider
// =============================================================================

#[derive(Debug)]
pub struct XAIProvider {
    pub(crate) endpoint: String,
    pub(crate) api_key: Option<String>,
    pub(crate) model: String,
    pub(crate) settings: XAISettings,
}

impl XAIProvider {
    pub fn new(endpoint: &str, api_key: Option<&str>, model: &str, settings: XAISettings) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            api_key: api_key.map(|s| s.to_string()),
            model: model.to_string(),
            settings,
        }
    }

    /// Whether this provider instance should use the Responses API.
    fn is_responses_mode(&self) -> bool {
        self.model.contains("multi-agent")
    }
}

#[async_trait]
impl CompletionProvider for XAIProvider {
    fn name(&self) -> &'static str {
        "xAI"
    }

    async fn send_completion(
        &self,
        client: &reqwest::Client,
        messages: &[serde_json::Value],
        tools: Option<&[serde_json::Value]>,
        max_tokens: u32,
        on_token: &(dyn Fn(StreamEvent) + Send + Sync),
        agent_id: &str,
    ) -> Result<AssembledResponse, StreamError> {
        if self.is_responses_mode() {
            self.send_responses_completion(client, messages, tools, max_tokens, on_token, agent_id)
                .await
        } else {
            self.send_chat_completion(client, messages, tools, max_tokens, on_token, agent_id)
                .await
        }
    }
}

impl XAIProvider {
    // =========================================================================
    // Chat Completions path (standard Grok models)
    // =========================================================================

    async fn send_chat_completion(
        &self,
        client: &reqwest::Client,
        messages: &[serde_json::Value],
        tools: Option<&[serde_json::Value]>,
        max_tokens: u32,
        on_token: &(dyn Fn(StreamEvent) + Send + Sync),
        agent_id: &str,
    ) -> Result<AssembledResponse, StreamError> {
        let url = format!("{}/chat/completions", self.endpoint.trim_end_matches('/'));

        let converted_messages = convert_content_for_xai(messages);

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": converted_messages,
            "stream": true,
            "stream_options": { "include_usage": true },
            "max_tokens": max_tokens,
        });

        // Only include sampling params for non-always-reasoning models
        if !is_always_reasoning_model(&self.model) {
            if let Some(temp) = self.settings.temperature {
                body["temperature"] = serde_json::json!(temp);
            }
        }

        // Tool calling
        if let Some(tools) = tools {
            if !tools.is_empty() {
                body["tools"] = serde_json::json!(tools);
            }
        }

        // Search parameters
        if self.settings.search_mode.should_include() {
            body["search_parameters"] = serde_json::json!({
                "mode": self.settings.search_mode.as_str()
            });
        }

        // Deferred mode
        if self.settings.deferred {
            body["deferred"] = serde_json::json!(true);
        }

        // Build request with xAI-specific headers
        let mut req = client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        // Cache affinity header
        if let Some(conv_id) = &self.settings.conversation_id {
            req = req.header("x-grok-conv-id", conv_id);
        }

        let response = req.send().await.map_err(StreamError::Network)?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            return Err(StreamError::RateLimited { retry_after });
        }
        if status.is_server_error() {
            return Err(StreamError::ServerError(status.as_u16()));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(StreamError::HttpError {
                status: status.as_u16(),
                body,
            });
        }

        // Parse Chat Completions SSE stream (same format as OpenAI)
        let mut content = String::new();
        let mut thinking_content = String::new();
        let mut tool_calls: HashMap<usize, PartialToolCall> = HashMap::new();
        let mut usage = None;
        let mut finish_reason = None;

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(StreamError::Network)?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim_end_matches('\r').to_string();
                buffer = buffer[line_end + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        continue;
                    }

                    let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data) else {
                        continue;
                    };

                    if let Some(u) = chunk.usage {
                        usage = Some(u);
                    }

                    if let Some(choices) = chunk.choices {
                        for choice in choices {
                            if let Some(fr) = choice.finish_reason {
                                finish_reason = Some(fr);
                            }
                            if let Some(delta) = choice.delta {
                                // Reasoning/thinking content (always-on for grok-4.3, grok-4)
                                if let Some(thinking) = delta.reasoning_content {
                                    thinking_content.push_str(&thinking);
                                    on_token(StreamEvent {
                                        agent_id: agent_id.to_string(),
                                        event_type: StreamEventType::Thinking,
                                        data: serde_json::json!({"text": thinking}),
                                    });
                                }

                                // Text content
                                if let Some(text) = delta.content {
                                    content.push_str(&text);
                                    on_token(StreamEvent {
                                        agent_id: agent_id.to_string(),
                                        event_type: StreamEventType::Token,
                                        data: serde_json::json!({"text": text}),
                                    });
                                }

                                // Tool calls (assembled from chunks)
                                if let Some(tcs) = delta.tool_calls {
                                    for tc in tcs {
                                        let idx = tc.index.unwrap_or(0);
                                        let entry = tool_calls.entry(idx).or_insert_with(|| {
                                            PartialToolCall {
                                                id: String::new(),
                                                name: String::new(),
                                                arguments: String::new(),
                                            }
                                        });

                                        if let Some(id) = tc.id {
                                            entry.id = id;
                                        }
                                        if let Some(func) = tc.function {
                                            if let Some(name) = func.name {
                                                entry.name = name;
                                            }
                                            if let Some(args) = func.arguments {
                                                entry.arguments.push_str(&args);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Emit done event
        on_token(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Done,
            data: serde_json::json!({}),
        });

        // Assemble tool calls
        let mut assembled_tool_calls: Vec<ToolCallRecord> = tool_calls
            .into_values()
            .map(|ptc| {
                let args = match serde_json::from_str::<serde_json::Value>(&ptc.arguments) {
                    Ok(v) if v.is_object() => v,
                    Ok(_) => serde_json::json!({}),
                    Err(e) => {
                        tracing::warn!(
                            tool = %ptc.name,
                            raw_args = %ptc.arguments.chars().take(200).collect::<String>(),
                            "Failed to parse xAI tool call arguments: {e}, using empty object"
                        );
                        serde_json::json!({})
                    }
                };
                ToolCallRecord {
                    id: ptc.id,
                    name: ptc.name,
                    arguments: args,
                }
            })
            .collect();
        assembled_tool_calls.sort_by_key(|tc| tc.id.clone());

        Ok(AssembledResponse {
            content,
            tool_calls: assembled_tool_calls,
            usage,
            finish_reason,
            thinking_content: if thinking_content.is_empty() {
                None
            } else {
                Some(thinking_content)
            },
        })
    }

    // =========================================================================
    // Responses API path (multi-agent models)
    // =========================================================================

    async fn send_responses_completion(
        &self,
        client: &reqwest::Client,
        messages: &[serde_json::Value],
        tools: Option<&[serde_json::Value]>,
        max_tokens: u32,
        on_token: &(dyn Fn(StreamEvent) + Send + Sync),
        agent_id: &str,
    ) -> Result<AssembledResponse, StreamError> {
        let url = format!("{}/responses", self.endpoint.trim_end_matches('/'));

        let converted_messages = convert_content_for_xai(messages);

        let mut body = serde_json::json!({
            "model": self.model,
            "input": converted_messages,
            "stream": true,
            "max_output_tokens": max_tokens,
            "store": false,
        });

        // Multi-agent models don't support custom client-side tools,
        // but if this is a non-multi-agent model using Responses API for
        // some reason, include them.
        if !self.model.contains("multi-agent") {
            if let Some(tools) = tools {
                if !tools.is_empty() {
                    body["tools"] = serde_json::json!(tools);
                }
            }
        }

        // Reasoning effort (only for multi-agent models that support it)
        if let Some(ref effort) = self.settings.reasoning_effort {
            body["reasoning"] = serde_json::json!({
                "effort": effort.as_str()
            });
        }

        let mut req = client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        // Cache affinity header
        if let Some(conv_id) = &self.settings.conversation_id {
            req = req.header("x-grok-conv-id", conv_id);
        }

        let response = req.send().await.map_err(StreamError::Network)?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            return Err(StreamError::RateLimited { retry_after });
        }
        if status.is_server_error() {
            return Err(StreamError::ServerError(status.as_u16()));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(StreamError::HttpError {
                status: status.as_u16(),
                body,
            });
        }

        // Parse xAI Responses SSE stream
        let mut content = String::new();
        let mut usage = None;
        let mut finish_reason = None;

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut current_event_type = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(StreamError::Network)?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim_end_matches('\r').to_string();
                buffer = buffer[line_end + 1..].to_string();

                // Track event type from `event:` lines
                if let Some(event_name) = line.strip_prefix("event: ") {
                    current_event_type = event_name.to_string();
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        continue;
                    }

                    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                        continue;
                    };

                    match current_event_type.as_str() {
                        "response.output_text.delta" => {
                            if let Some(delta) = event.get("delta").and_then(|v| v.as_str()) {
                                content.push_str(delta);
                                on_token(StreamEvent {
                                    agent_id: agent_id.to_string(),
                                    event_type: StreamEventType::Token,
                                    data: serde_json::json!({"text": delta}),
                                });
                            }
                        }
                        "response.completed" => {
                            finish_reason = Some("stop".to_string());
                            if let Some(resp_usage) = event.pointer("/response/usage") {
                                let input = resp_usage
                                    .get("input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .map(|v| v as u32);
                                let output = resp_usage
                                    .get("output_tokens")
                                    .and_then(|v| v.as_u64())
                                    .map(|v| v as u32);
                                let total = resp_usage
                                    .get("total_tokens")
                                    .and_then(|v| v.as_u64())
                                    .map(|v| v as u32);
                                usage = Some(UsageInfo {
                                    prompt_tokens: input,
                                    completion_tokens: output,
                                    total_tokens: total,
                                });
                            }
                        }
                        _ => {
                            // response.created, response.in_progress, etc.
                        }
                    }

                    current_event_type.clear();
                }

                if line.is_empty() {
                    current_event_type.clear();
                }
            }
        }

        on_token(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Done,
            data: serde_json::json!({}),
        });

        Ok(AssembledResponse {
            content,
            tool_calls: vec![],
            usage,
            finish_reason,
            thinking_content: None,
        })
    }
}

// =============================================================================
// Image content block conversion
// =============================================================================

/// Convert internal image content blocks to xAI-compatible format (same as OpenAI image_url).
fn convert_content_for_xai(messages: &[serde_json::Value]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .map(|msg| {
            let mut m = msg.clone();
            if let Some(content) = msg.get("content") {
                if let Some(blocks) = content.as_array() {
                    let converted: Vec<serde_json::Value> = blocks
                        .iter()
                        .filter_map(|block| {
                            let block_type = block.get("type")?.as_str()?;
                            match block_type {
                                "text" => Some(block.clone()),
                                "image" => {
                                    let data = block.get("data")?.as_str()?;
                                    let media_type = block.get("media_type")?.as_str()?;
                                    Some(serde_json::json!({
                                        "type": "image_url",
                                        "image_url": {
                                            "url": format!("data:{};base64,{}", media_type, data)
                                        }
                                    }))
                                }
                                _ => Some(block.clone()),
                            }
                        })
                        .collect();
                    m["content"] = serde_json::Value::Array(converted);
                }
            }
            m
        })
        .collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xai_provider_construction() {
        let settings = XAISettings {
            model: "grok-4.20-multi-agent".to_string(),
            ..Default::default()
        };
        let provider = XAIProvider::new(
            "https://api.x.ai/v1",
            Some("xai-test-key"),
            "grok-4.20-multi-agent",
            settings,
        );
        assert_eq!(provider.name(), "xAI");
        assert_eq!(provider.endpoint, "https://api.x.ai/v1");
        assert_eq!(provider.model, "grok-4.20-multi-agent");
        assert!(provider.is_responses_mode());
    }

    #[test]
    fn test_xai_provider_standard_mode() {
        let settings = XAISettings {
            model: "grok-4.3".to_string(),
            ..Default::default()
        };
        let provider = XAIProvider::new(
            "https://api.x.ai/v1",
            Some("xai-test-key"),
            "grok-4.3",
            settings,
        );
        assert_eq!(provider.name(), "xAI");
        assert!(!provider.is_responses_mode());
    }

    #[test]
    fn test_xai_provider_no_key() {
        let settings = XAISettings::default();
        let provider = XAIProvider::new("https://api.x.ai/v1", None, "grok-4.3", settings);
        assert_eq!(provider.api_key, None);
    }

    #[test]
    fn test_xai_provider_with_conversation_id() {
        let settings = XAISettings {
            model: "grok-4.3".to_string(),
            conversation_id: Some("conv-abc-123".to_string()),
            ..Default::default()
        };
        let provider = XAIProvider::new("https://api.x.ai/v1", Some("key"), "grok-4.3", settings);
        assert_eq!(
            provider.settings.conversation_id.as_deref(),
            Some("conv-abc-123")
        );
    }

    #[test]
    fn test_xai_routing_multi_agent_uses_responses() {
        let settings = XAISettings::default();
        let provider = XAIProvider::new(
            "https://api.x.ai/v1",
            None,
            "grok-4.20-multi-agent",
            settings,
        );
        assert!(provider.is_responses_mode());
    }

    #[test]
    fn test_xai_routing_standard_uses_chat() {
        for model in ["grok-4.3", "grok-4", "grok-4-fast", "grok-4.1-fast"] {
            let settings = XAISettings {
                model: model.to_string(),
                ..Default::default()
            };
            let provider = XAIProvider::new("https://api.x.ai/v1", None, model, settings);
            assert!(
                !provider.is_responses_mode(),
                "{model} should use Chat Completions"
            );
        }
    }

    #[test]
    fn test_parse_chat_completion_chunk() {
        let data = r#"{"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(data).unwrap();
        let choices = chunk.choices.unwrap();
        let content = choices[0].delta.as_ref().unwrap().content.as_ref().unwrap();
        assert_eq!(content, "Hello");
    }

    #[test]
    fn test_parse_reasoning_content_chunk() {
        let data = r#"{"choices":[{"delta":{"reasoning_content":"Let me think..."},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(data).unwrap();
        let choices = chunk.choices.unwrap();
        let thinking = choices[0]
            .delta
            .as_ref()
            .unwrap()
            .reasoning_content
            .as_ref()
            .unwrap();
        assert_eq!(thinking, "Let me think...");
    }

    #[test]
    fn test_convert_content_for_xai_image() {
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "describe this"},
                {"type": "image", "data": "abc", "media_type": "image/png"}
            ]
        })];
        let result = convert_content_for_xai(&messages);
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["url"], "data:image/png;base64,abc");
    }
}
