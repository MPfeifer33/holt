//! OpenAI-compatible ChatCompletions provider.
//!
//! Handles streaming SSE responses from any OpenAI-compatible API endpoint
//! (OpenAI, local models via Ollama/LM Studio/llama-server, etc.).

use async_trait::async_trait;
use futures::StreamExt;
use std::collections::HashMap;

use crate::providers::types::*;
use crate::runtime::agent::ToolCallRecord;

// =============================================================================
// SSE deserialization structs
// =============================================================================

/// OpenAI-compatible ChatCompletion chunk from SSE stream.
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
    /// Reasoning/thinking tokens from reasoning models (xAI Grok, OpenAI o1, etc.)
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
// Image content block conversion
// =============================================================================

/// Convert internal image content blocks to OpenAI image_url format and strip
/// Anthropic-specific cache sentinel characters from all message content.
/// Handles both string content and array content (convert image blocks).
fn convert_content_for_openai(messages: &[serde_json::Value]) -> Vec<serde_json::Value> {
    use crate::runtime::cache::sentinels::strip_sentinels;

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
                                "text" => {
                                    // Strip sentinels from text blocks
                                    let text = block.get("text")?.as_str()?;
                                    let cleaned = strip_sentinels(text);
                                    Some(serde_json::json!({"type": "text", "text": cleaned}))
                                }
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
                } else if let Some(text) = content.as_str() {
                    // Strip sentinels from plain string content
                    m["content"] = serde_json::Value::String(strip_sentinels(text));
                }
            }
            m
        })
        .collect()
}

// =============================================================================
// OpenAIProvider
// =============================================================================

#[derive(Debug)]
pub struct OpenAIProvider {
    pub(crate) endpoint: String,
    pub(crate) api_key: Option<String>,
    pub(crate) model: String,
}

impl OpenAIProvider {
    pub fn new(endpoint: &str, api_key: Option<&str>, model: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            api_key: api_key.map(|s| s.to_string()),
            model: model.to_string(),
        }
    }
}

#[async_trait]
impl CompletionProvider for OpenAIProvider {
    fn name(&self) -> &'static str {
        "OpenAI-compatible"
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
        let url = format!("{}/chat/completions", self.endpoint.trim_end_matches('/'));

        // Convert internal image content blocks to OpenAI image_url format
        let converted_messages = convert_content_for_openai(messages);

        // OpenAI deprecated `max_tokens` in favor of `max_completion_tokens`.
        // o-series reasoning models and GPT-5 REJECT `max_tokens` outright
        // (400 "Unsupported parameter"). The legacy chat models that only
        // understood `max_tokens` (gpt-4o, gpt-4.1, o4-mini) were retired
        // in February 2026, so every model currently live on api.openai.com
        // accepts `max_completion_tokens`. For OpenAI-compatible endpoints
        // that haven't adopted the new field (Grok, Ollama, Groq, Together,
        // etc.), we continue to send `max_tokens`.
        let token_field = if self.endpoint.contains("api.openai.com") {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": converted_messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        body[token_field] = serde_json::json!(max_tokens);

        if let Some(tools) = tools {
            if !tools.is_empty() {
                body["tools"] = serde_json::json!(tools);
            }
        }

        let mut req = client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
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

        // Parse SSE stream
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

            // Process complete SSE lines
            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim_end_matches('\r').to_string();
                buffer = buffer[line_end + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        continue;
                    }

                    match serde_json::from_str::<ChatCompletionChunk>(data) {
                        Err(e) => {
                            // Check if this is an error event from the provider
                            if let Ok(error_obj) = serde_json::from_str::<serde_json::Value>(data) {
                                if let Some(err) = error_obj.get("error") {
                                    let error_msg = err
                                        .get("message")
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("Unknown error from provider");
                                    tracing::warn!(
                                        agent_id,
                                        error = error_msg,
                                        "SSE error event from OpenAI-compatible provider"
                                    );
                                    return Err(StreamError::ApiError(format!(
                                        "Provider error: {}",
                                        error_msg
                                    )));
                                }
                            }
                            // Not an error event, just a malformed chunk — log and continue
                            tracing::debug!(
                                agent_id,
                                parse_error = %e,
                                data = %data.chars().take(200).collect::<String>(),
                                "Failed to parse SSE chunk, skipping"
                            );
                        }
                        Ok(chunk) => {
                            if let Some(u) = chunk.usage {
                                usage = Some(u);
                            }

                            if let Some(choices) = chunk.choices {
                                for choice in choices {
                                    if let Some(fr) = choice.finish_reason {
                                        finish_reason = Some(fr);
                                    }
                                    if let Some(delta) = choice.delta {
                                        // Reasoning/thinking content (xAI Grok, OpenAI o1)
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
                                                let entry =
                                                    tool_calls.entry(idx).or_insert_with(|| {
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
            }
        }

        // Process any remaining data in buffer (last chunk may not end with \n)
        let remaining = buffer.trim();
        if let Some(data) = remaining.strip_prefix("data: ") {
            if data != "[DONE]" {
                match serde_json::from_str::<ChatCompletionChunk>(data) {
                    Err(e) => {
                        if let Ok(error_obj) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(err) = error_obj.get("error") {
                                let error_msg = err
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("Unknown error from provider");
                                tracing::warn!(
                                    agent_id,
                                    error = error_msg,
                                    "SSE error event from OpenAI-compatible provider (remainder)"
                                );
                                return Err(StreamError::ApiError(format!(
                                    "Provider error: {}",
                                    error_msg
                                )));
                            }
                        }
                        tracing::debug!(
                            agent_id,
                            parse_error = %e,
                            "Failed to parse remaining SSE data, skipping"
                        );
                    }
                    Ok(chunk) => {
                        if let Some(u) = chunk.usage {
                            usage = Some(u);
                        }
                        if let Some(choices) = chunk.choices {
                            for choice in choices {
                                if let Some(fr) = choice.finish_reason {
                                    finish_reason = Some(fr);
                                }
                                if let Some(delta) = choice.delta {
                                    if let Some(thinking) = delta.reasoning_content {
                                        thinking_content.push_str(&thinking);
                                        on_token(StreamEvent {
                                            agent_id: agent_id.to_string(),
                                            event_type: StreamEventType::Thinking,
                                            data: serde_json::json!({"text": thinking}),
                                        });
                                    }
                                    if let Some(text) = delta.content {
                                        content.push_str(&text);
                                        on_token(StreamEvent {
                                            agent_id: agent_id.to_string(),
                                            event_type: StreamEventType::Token,
                                            data: serde_json::json!({"text": text}),
                                        });
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
                    Ok(v) => {
                        tracing::warn!(
                            tool = %ptc.name,
                            "Tool call arguments parsed as non-object JSON ({}), wrapping in empty object",
                            v.to_string().chars().take(100).collect::<String>()
                        );
                        serde_json::json!({})
                    }
                    Err(e) => {
                        tracing::warn!(
                            tool = %ptc.name,
                            raw_args = %ptc.arguments.chars().take(200).collect::<String>(),
                            "Failed to parse tool call arguments as JSON: {e}, using empty object"
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
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sse_chunk() {
        let data = r#"{"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(data).unwrap();
        let choices = chunk.choices.unwrap();
        let content = choices[0].delta.as_ref().unwrap().content.as_ref().unwrap();
        assert_eq!(content, "Hello");
    }

    #[test]
    fn test_parse_tool_call_chunk() {
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_123","type":"function","function":{"name":"bash","arguments":""}}]},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(data).unwrap();
        let choices = chunk.choices.unwrap();
        let tc = &choices[0]
            .delta
            .as_ref()
            .unwrap()
            .tool_calls
            .as_ref()
            .unwrap()[0];
        assert_eq!(tc.id.as_ref().unwrap(), "call_123");
        assert_eq!(tc.function.as_ref().unwrap().name.as_ref().unwrap(), "bash");
    }

    #[test]
    fn test_openai_provider_construction() {
        let provider = OpenAIProvider::new("https://api.openai.com/v1", Some("sk-test"), "gpt-4");
        assert_eq!(provider.name(), "OpenAI-compatible");
        assert_eq!(provider.endpoint, "https://api.openai.com/v1");
        assert_eq!(provider.api_key.as_deref(), Some("sk-test"));
        assert_eq!(provider.model, "gpt-4");
    }

    #[test]
    fn test_openai_provider_no_key() {
        let provider = OpenAIProvider::new("http://localhost:11434/v1", None, "llama3");
        assert_eq!(provider.api_key, None);
    }

    #[test]
    fn test_convert_content_for_openai_image_blocks() {
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "describe this"},
                {"type": "image", "data": "abc", "media_type": "image/png"}
            ]
        })];
        let result = convert_content_for_openai(&messages);
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["url"], "data:image/png;base64,abc");
    }

    #[test]
    fn test_convert_content_for_openai_string_passthrough() {
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": "hello"
        })];
        let result = convert_content_for_openai(&messages);
        assert_eq!(result[0]["content"], "hello");
    }
}
