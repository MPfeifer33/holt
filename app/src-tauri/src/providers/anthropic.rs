// providers/anthropic.rs — Anthropic Messages API provider (OAuth path).
//
// PRESERVATION CONTRACT: This file is a byte-for-byte move of runtime/anthropic.rs
// wrapped in the CompletionProvider trait. The OAuth headers, Claude Code identity
// headers, message conversion, tool conversion, and SSE parsing logic are IDENTICAL
// to the working runtime/anthropic.rs. Do not modify without running all 4 tests
// AND a manual smoke test.

use futures::StreamExt;

use crate::providers::types::*;
use crate::runtime::agent::ToolCallRecord;
use crate::runtime::cache::sentinels;

// =============================================================================
// Constants — identical to runtime/anthropic.rs
// =============================================================================

/// Anthropic API endpoint constant.
pub const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";

/// Anthropic API version header.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Required beta headers for OAuth token authentication.
const OAUTH_BETA: &str = "oauth-2025-04-20";
const CLAUDE_CODE_BETA: &str = "claude-code-20250219";

/// Claude Code identity headers required by Anthropic API for OAuth access.
const CLAUDE_CLI_USER_AGENT: &str = "claude-cli/2.1.81";
const CLAUDE_CODE_SYSTEM_PREFIX: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

// =============================================================================
// Message/tool conversion helpers — identical to runtime/anthropic.rs
// =============================================================================

/// Convert OpenAI-format tool definitions to Anthropic format.
pub fn convert_tools_to_anthropic(tools: &[serde_json::Value]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .filter_map(|tool| {
            let func = tool.get("function")?;
            let name = func.get("name")?.as_str()?;
            let description = func.get("description")?.as_str().unwrap_or("");
            let parameters = func
                .get("parameters")
                .cloned()
                .unwrap_or(serde_json::json!({"type": "object", "properties": {}}));
            Some(serde_json::json!({
                "name": name,
                "description": description,
                "input_schema": parameters,
            }))
        })
        .collect()
}

/// Split the augmented system prompt on cache sentinels into content blocks
/// with appropriate `cache_control` annotations.
///
/// The prompt is expected to contain up to two sentinels:
/// - CACHE_SENTINEL_1HR after session-stable content (system prompt + tools + persona)
/// - CACHE_SENTINEL_5M after semi-stable content (user notes + skills)
///
/// Regions before each sentinel get `cache_control: {type: "ephemeral"}`.
/// The final region (dynamic memory) gets no cache_control.
fn build_cached_system_blocks(system_prompt: &str) -> Vec<serde_json::Value> {
    let regions = sentinels::split_at_sentinels(system_prompt);

    regions
        .into_iter()
        .filter(|r| !r.content.trim().is_empty())
        .map(|r| {
            let mut block = serde_json::json!({
                "type": "text",
                "text": r.content,
            });
            // Stable and semi-stable regions get cache_control
            match r.cache_hint {
                sentinels::CacheHint::LongTtl | sentinels::CacheHint::ShortTtl => {
                    block["cache_control"] = serde_json::json!({"type": "ephemeral"});
                }
                // Dynamic (NoCache) and trailing (None) regions: no cache_control
                _ => {}
            }
            block
        })
        .collect()
}

/// Convert conversation messages to Anthropic format.
/// Returns (system_prompt, messages) — system is a top-level field in Anthropic API.
pub fn convert_messages_to_anthropic(
    messages: &[serde_json::Value],
) -> (String, Vec<serde_json::Value>) {
    let mut system_prompt = String::new();
    let mut anthropic_messages = Vec::new();

    for msg in messages {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");

        match role {
            "system" => {
                if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                    if !system_prompt.is_empty() {
                        system_prompt.push_str("\n\n");
                    }
                    system_prompt.push_str(content);
                }
            }
            "assistant" => {
                let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
                let thinking = msg.get("thinking_content").and_then(|t| t.as_str());
                let tool_calls = msg.get("tool_calls").and_then(|tc| tc.as_array());

                let mut blocks: Vec<serde_json::Value> = Vec::new();

                // Preserve thinking blocks (required for multi-turn thinking)
                if let Some(thinking_text) = thinking {
                    if !thinking_text.is_empty() {
                        blocks.push(serde_json::json!({
                            "type": "thinking",
                            "thinking": thinking_text,
                        }));
                    }
                }

                if !content.is_empty() {
                    blocks.push(serde_json::json!({
                        "type": "text",
                        "text": content,
                    }));
                }

                // Convert tool calls to tool_use blocks
                if let Some(calls) = tool_calls {
                    for tc in calls {
                        if let (Some(id), Some(name), Some(args)) = (
                            tc.get("id").and_then(|i| i.as_str()),
                            tc.get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|n| n.as_str()),
                            tc.get("function").and_then(|f| f.get("arguments")),
                        ) {
                            let input: serde_json::Value = if let Some(s) = args.as_str() {
                                serde_json::from_str(s).unwrap_or(serde_json::json!({}))
                            } else {
                                args.clone()
                            };
                            blocks.push(serde_json::json!({
                                "type": "tool_use",
                                "id": id,
                                "name": name,
                                "input": input,
                            }));
                        }
                    }
                }

                if !blocks.is_empty() {
                    anthropic_messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": blocks,
                    }));
                }
            }
            "tool" => {
                let tool_call_id = msg
                    .get("tool_call_id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("");
                let content_value = msg.get("content").cloned().unwrap_or(serde_json::json!(""));
                let tool_content = if let Some(text) = content_value.as_str() {
                    serde_json::json!(text)
                } else if let Some(blocks) = content_value.as_array() {
                    // Tool returned multimodal content — convert image blocks to Anthropic format
                    let converted: Vec<serde_json::Value> = blocks
                        .iter()
                        .filter_map(|block| {
                            let block_type = block.get("type")?.as_str()?;
                            match block_type {
                                "text" => Some(serde_json::json!({
                                    "type": "text",
                                    "text": block.get("text")?.as_str()?,
                                })),
                                "image" => Some(serde_json::json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": block.get("media_type")?.as_str()?,
                                        "data": block.get("data")?.as_str()?,
                                    }
                                })),
                                _ => None,
                            }
                        })
                        .collect();
                    serde_json::Value::Array(converted)
                } else {
                    serde_json::json!(content_value.to_string())
                };
                let tool_result = serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tool_call_id,
                    "content": tool_content,
                });

                // Merge consecutive tool results into a single user message
                // (Anthropic requires all tool_results for a turn in one message)
                if let Some(last) = anthropic_messages.last_mut() {
                    if last.get("role").and_then(|r| r.as_str()) == Some("user") {
                        if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                            if arr.iter().all(|item| {
                                item.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                            }) {
                                arr.push(tool_result);
                                continue;
                            }
                        }
                    }
                }
                anthropic_messages.push(serde_json::json!({
                    "role": "user",
                    "content": [tool_result],
                }));
            }
            "user" => {
                let content_value = msg.get("content").cloned().unwrap_or(serde_json::json!(""));
                let anthropic_content = if let Some(text) = content_value.as_str() {
                    // Simple text message (existing behavior)
                    serde_json::json!(text)
                } else if let Some(blocks) = content_value.as_array() {
                    // Multimodal content blocks — convert to Anthropic format
                    let converted: Vec<serde_json::Value> = blocks
                        .iter()
                        .filter_map(|block| {
                            let block_type = block.get("type")?.as_str()?;
                            match block_type {
                                "text" => Some(serde_json::json!({
                                    "type": "text",
                                    "text": block.get("text")?.as_str()?,
                                })),
                                "image" => Some(serde_json::json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": block.get("media_type")?.as_str()?,
                                        "data": block.get("data")?.as_str()?,
                                    }
                                })),
                                _ => None,
                            }
                        })
                        .collect();
                    serde_json::Value::Array(converted)
                } else {
                    serde_json::json!("")
                };
                anthropic_messages.push(serde_json::json!({
                    "role": "user",
                    "content": anthropic_content,
                }));
            }
            _ => {}
        }
    }

    // Post-process: enforce strict user/assistant alternation.
    // Merge consecutive same-role messages into one.
    let mut merged: Vec<serde_json::Value> = Vec::new();
    for msg in anthropic_messages {
        let role = msg
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();
        let prev_role = merged
            .last()
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();

        if role == prev_role {
            // Merge into the previous message of the same role
            if let Some(prev) = merged.last_mut() {
                let prev_content = prev
                    .get("content")
                    .cloned()
                    .unwrap_or(serde_json::json!(""));
                let new_content = msg.get("content").cloned().unwrap_or(serde_json::json!(""));

                fn to_blocks(v: serde_json::Value) -> Vec<serde_json::Value> {
                    match v {
                        serde_json::Value::Array(arr) => arr,
                        serde_json::Value::String(s) => {
                            vec![serde_json::json!({"type": "text", "text": s})]
                        }
                        other => vec![other],
                    }
                }

                let mut blocks = to_blocks(prev_content);
                blocks.extend(to_blocks(new_content));
                prev["content"] = serde_json::Value::Array(blocks);
            }
        } else {
            merged.push(msg);
        }
    }

    (system_prompt, merged)
}

// =============================================================================
// AnthropicProvider
// =============================================================================

#[derive(Debug)]
pub struct AnthropicProvider {
    pub(crate) bearer_token: String,
    pub(crate) model: String,
    pub(crate) thinking_enabled: bool,
    pub(crate) thinking_budget: u32,
}

impl AnthropicProvider {
    pub fn new(
        bearer_token: &str,
        model: &str,
        thinking_enabled: bool,
        thinking_budget: u32,
    ) -> Self {
        Self {
            bearer_token: bearer_token.to_string(),
            model: model.to_string(),
            thinking_enabled,
            thinking_budget,
        }
    }
}

#[async_trait::async_trait]
impl CompletionProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "Anthropic Messages"
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
        let (system_prompt, mut anthropic_messages) = convert_messages_to_anthropic(messages);
        let mut anthropic_tools = tools.map(convert_tools_to_anthropic);

        // Anthropic: max_tokens covers ALL output (thinking + response combined).
        // With thinking enabled, budget_tokens is a sub-allocation within max_tokens.
        // Cap at 16384 without thinking, 32000 with thinking.
        let effective_max_tokens = if self.thinking_enabled {
            max_tokens.max(self.thinking_budget + 4096).min(32000)
        } else {
            max_tokens.min(16384)
        };

        // OAuth requires system prompt to be EXACTLY the Claude Code prefix.
        // Any custom system/persona content must go as a prefixed user message.
        // Split on cache sentinels to create multiple content blocks with cache_control,
        // enabling Anthropic's prompt caching to cache the stable prefix independently
        // of dynamic content (memory, skills) that changes every turn.
        if !system_prompt.is_empty() {
            let cache_blocks = build_cached_system_blocks(&system_prompt);

            let first_is_user = anthropic_messages
                .first()
                .and_then(|m| m.get("role"))
                .and_then(|r| r.as_str())
                == Some("user");

            if first_is_user {
                if let Some(first_msg) = anthropic_messages.first_mut() {
                    let existing_content = first_msg.get("content").cloned();
                    let mut blocks = cache_blocks;
                    // Append the existing first user message content as a final block
                    match existing_content {
                        Some(serde_json::Value::String(s)) if !s.is_empty() => {
                            blocks.push(serde_json::json!({"type": "text", "text": s}));
                        }
                        Some(serde_json::Value::Array(arr)) => {
                            blocks.extend(arr);
                        }
                        _ => {}
                    }
                    first_msg["content"] = serde_json::Value::Array(blocks);
                }
            } else {
                anthropic_messages.insert(
                    0,
                    serde_json::json!({
                        "role": "user",
                        "content": cache_blocks,
                    }),
                );
            }
        }

        // Debug: dump message structure before sending
        for (i, msg) in anthropic_messages.iter().enumerate() {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("?");
            let content = msg.get("content");
            let desc = match content {
                Some(serde_json::Value::Array(arr)) => {
                    let types: Vec<String> = arr
                        .iter()
                        .map(|b| {
                            let t = b.get("type").and_then(|t| t.as_str()).unwrap_or("?");
                            match t {
                                "tool_use" => format!(
                                    "USE:{}",
                                    b.get("id")
                                        .and_then(|i| i.as_str())
                                        .unwrap_or("?")
                                        .chars()
                                        .take(12)
                                        .collect::<String>()
                                ),
                                "tool_result" => format!(
                                    "RES:{}",
                                    b.get("tool_use_id")
                                        .and_then(|i| i.as_str())
                                        .unwrap_or("?")
                                        .chars()
                                        .take(12)
                                        .collect::<String>()
                                ),
                                "thinking" => "think".to_string(),
                                _ => t.to_string(),
                            }
                        })
                        .collect();
                    format!("{:?}", types)
                }
                Some(serde_json::Value::String(s)) => format!("text({}ch)", s.len()),
                _ => "?".to_string(),
            };
            tracing::info!("anthropic msg[{}]: {} -> {}", i, role, desc);
        }

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": effective_max_tokens,
            "stream": true,
            "system": [{
                "type": "text",
                "text": CLAUDE_CODE_SYSTEM_PREFIX,
                "cache_control": { "type": "ephemeral" }
            }],
            "messages": anthropic_messages,
        });

        if let Some(ref mut tools) = anthropic_tools {
            // Add cache_control to last tool for prompt cache reuse
            if let Some(last_tool) = tools.last_mut() {
                last_tool["cache_control"] = serde_json::json!({ "type": "ephemeral" });
            }
            if !tools.is_empty() {
                body["tools"] = serde_json::json!(tools);
            }
        }

        if self.thinking_enabled {
            body["thinking"] = serde_json::json!({
                "type": "enabled",
                "budget_tokens": self.thinking_budget,
            });
        }

        // Build beta header: OAuth + Claude Code identity + optional thinking
        let mut betas = vec![
            CLAUDE_CODE_BETA.to_string(),
            OAUTH_BETA.to_string(),
            "prompt-caching-2024-07-31".to_string(),
        ];
        if self.thinking_enabled {
            betas.push("interleaved-thinking-2025-05-14".to_string());
        }
        let anthropic_beta = betas.join(",");

        tracing::info!(
            model = %self.model,
            effective_max_tokens,
            thinking_enabled = self.thinking_enabled,
            thinking_budget = self.thinking_budget,
            message_count = anthropic_messages.len(),
            has_system = !system_prompt.is_empty(),
            has_tools = anthropic_tools.as_ref().map(|t| t.len()).unwrap_or(0),
            "Anthropic request"
        );

        let request = client
            .post(ANTHROPIC_API_URL)
            .header("Authorization", format!("Bearer {}", self.bearer_token))
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("anthropic-beta", &anthropic_beta)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .header("anthropic-dangerous-direct-browser-access", "true")
            .header("user-agent", CLAUDE_CLI_USER_AGENT)
            .header("x-app", "cli");

        let resp = request
            .json(&body)
            .send()
            .await
            .map_err(StreamError::Network)?;

        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            return Err(StreamError::RateLimited { retry_after });
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(StreamError::ApiError(
                "401 Unauthorized — OAuth token may be expired".to_string(),
            ));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            tracing::error!(status = %status, "Anthropic API error response: {}", body);
            tracing::error!(
                "Request was: model={}, max_tokens={}, thinking={}, messages={}",
                self.model,
                effective_max_tokens,
                self.thinking_enabled,
                anthropic_messages.len()
            );
            return Err(StreamError::ApiError(format!(
                "Anthropic API error {}: {}",
                status, body
            )));
        }

        // Parse SSE stream
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut text_content = String::new();
        let mut thinking_content = String::new();
        let mut tool_calls: Vec<ToolCallRecord> = Vec::new();
        let mut usage_info: Option<UsageInfo> = None;
        let mut finish_reason: Option<String> = None;

        // Track current content block for accumulation
        let mut current_block_type: Option<String> = None;
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_input = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| StreamError::ApiError(e.to_string()))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete SSE lines
            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim().to_string();
                buffer = buffer[line_end + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        continue;
                    }

                    let event: serde_json::Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");

                    match event_type {
                        "content_block_start" => {
                            if let Some(block) = event.get("content_block") {
                                let block_type =
                                    block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                current_block_type = Some(block_type.to_string());

                                if block_type == "tool_use" {
                                    current_tool_id = block
                                        .get("id")
                                        .and_then(|i| i.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    current_tool_name = block
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    current_tool_input.clear();
                                }
                            }
                        }
                        "content_block_delta" => {
                            if let Some(delta) = event.get("delta") {
                                let delta_type =
                                    delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                match delta_type {
                                    "text_delta" => {
                                        if let Some(text) =
                                            delta.get("text").and_then(|t| t.as_str())
                                        {
                                            text_content.push_str(text);
                                            on_token(StreamEvent {
                                                agent_id: agent_id.to_string(),
                                                event_type: StreamEventType::Token,
                                                data: serde_json::json!({"text": text}),
                                            });
                                        }
                                    }
                                    "thinking_delta" => {
                                        if let Some(thinking) =
                                            delta.get("thinking").and_then(|t| t.as_str())
                                        {
                                            thinking_content.push_str(thinking);
                                            on_token(StreamEvent {
                                                agent_id: agent_id.to_string(),
                                                event_type: StreamEventType::Thinking,
                                                data: serde_json::json!({"text": thinking}),
                                            });
                                        }
                                    }
                                    "input_json_delta" => {
                                        if let Some(partial) =
                                            delta.get("partial_json").and_then(|p| p.as_str())
                                        {
                                            current_tool_input.push_str(partial);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        "content_block_stop" => {
                            if current_block_type.as_deref() == Some("tool_use") {
                                let arguments: serde_json::Value =
                                    serde_json::from_str(&current_tool_input)
                                        .unwrap_or(serde_json::json!({}));
                                tool_calls.push(ToolCallRecord {
                                    id: current_tool_id.clone(),
                                    name: current_tool_name.clone(),
                                    arguments,
                                });
                            }
                            current_block_type = None;
                        }
                        "message_delta" => {
                            if let Some(delta) = event.get("delta") {
                                finish_reason = delta
                                    .get("stop_reason")
                                    .and_then(|s| s.as_str())
                                    .map(|s| s.to_string());
                            }
                            if let Some(usage) = event.get("usage") {
                                usage_info = Some(UsageInfo {
                                    prompt_tokens: usage
                                        .get("input_tokens")
                                        .and_then(|t| t.as_u64())
                                        .map(|t| t as u32),
                                    completion_tokens: usage
                                        .get("output_tokens")
                                        .and_then(|t| t.as_u64())
                                        .map(|t| t as u32),
                                    total_tokens: None,
                                });
                            }
                        }
                        "message_start" => {
                            if let Some(message) = event.get("message") {
                                if let Some(usage) = message.get("usage") {
                                    let input = usage
                                        .get("input_tokens")
                                        .and_then(|t| t.as_u64())
                                        .map(|t| t as u32);
                                    if usage_info.is_none() {
                                        usage_info = Some(UsageInfo {
                                            prompt_tokens: input,
                                            completion_tokens: None,
                                            total_tokens: None,
                                        });
                                    }
                                }
                            }
                        }
                        "message_stop" | "ping" => {}
                        "error" => {
                            let error_msg = event
                                .get("error")
                                .and_then(|e| e.get("message"))
                                .and_then(|m| m.as_str())
                                .unwrap_or("Unknown Anthropic API error");
                            return Err(StreamError::ApiError(format!(
                                "Anthropic error: {}",
                                error_msg
                            )));
                        }
                        _ => {}
                    }
                }
            }
        }

        // Emit Done event (required by frontend to finalize message display)
        on_token(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Done,
            data: serde_json::json!({}),
        });

        Ok(AssembledResponse {
            content: text_content,
            tool_calls,
            usage: usage_info,
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
// Tests — identical to runtime/anthropic.rs tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_tools_to_anthropic() {
        let openai_tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            }
        })];

        let anthropic = convert_tools_to_anthropic(&openai_tools);
        assert_eq!(anthropic.len(), 1);
        assert_eq!(anthropic[0]["name"], "read_file");
        assert_eq!(anthropic[0]["description"], "Read a file");
        assert!(anthropic[0]["input_schema"]["properties"]["path"].is_object());
        assert!(anthropic[0].get("function").is_none());
    }

    #[test]
    fn test_convert_messages_system_extracted() {
        let messages = vec![
            serde_json::json!({"role": "system", "content": "You are helpful."}),
            serde_json::json!({"role": "user", "content": "Hello"}),
        ];
        let (system, msgs) = convert_messages_to_anthropic(&messages);
        assert_eq!(system, "You are helpful.");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    #[test]
    fn test_convert_tool_result_becomes_user_message() {
        let messages = vec![serde_json::json!({
            "role": "tool",
            "tool_call_id": "tc_123",
            "content": "file contents here"
        })];
        let (_, msgs) = convert_messages_to_anthropic(&messages);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[0]["content"][0]["tool_use_id"], "tc_123");
    }

    #[test]
    fn test_convert_assistant_with_thinking_preserved() {
        let messages = vec![serde_json::json!({
            "role": "assistant",
            "content": "The answer is 42.",
            "thinking_content": "Let me reason through this..."
        })];
        let (_, msgs) = convert_messages_to_anthropic(&messages);
        assert_eq!(msgs.len(), 1);
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "Let me reason through this...");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "The answer is 42.");
    }

    #[test]
    fn test_anthropic_provider_construction() {
        let provider =
            AnthropicProvider::new("test-bearer-token", "claude-sonnet-4-20250514", true, 10000);
        assert_eq!(provider.name(), "Anthropic Messages");
        assert_eq!(provider.model, "claude-sonnet-4-20250514");
        assert!(provider.thinking_enabled);
        assert_eq!(provider.thinking_budget, 10000);
    }

    #[test]
    fn test_convert_messages_user_with_image_blocks() {
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "What is this?"},
                {"type": "image", "data": "abc123", "media_type": "image/png"}
            ]
        })];
        let (_, result) = convert_messages_to_anthropic(&messages);
        assert_eq!(result.len(), 1);
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["data"], "abc123");
    }

    #[test]
    fn test_convert_messages_user_plain_text_unchanged() {
        let messages = vec![serde_json::json!({
            "role": "user",
            "content": "Hello"
        })];
        let (_, result) = convert_messages_to_anthropic(&messages);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["content"], "Hello");
    }

    #[test]
    fn test_build_cached_system_blocks_with_sentinels() {
        use crate::runtime::cache::sentinels::{CACHE_SENTINEL_1HR, CACHE_SENTINEL_5M};

        let prompt = format!(
            "system prompt + tools + persona{}user notes + skills{}memory section",
            CACHE_SENTINEL_1HR, CACHE_SENTINEL_5M
        );
        let blocks = build_cached_system_blocks(&prompt);

        assert_eq!(blocks.len(), 3);

        // Block 1: stable content with cache_control
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "system prompt + tools + persona");
        assert_eq!(blocks[0]["cache_control"]["type"], "ephemeral");

        // Block 2: semi-stable content with cache_control
        assert_eq!(blocks[1]["type"], "text");
        assert_eq!(blocks[1]["text"], "user notes + skills");
        assert_eq!(blocks[1]["cache_control"]["type"], "ephemeral");

        // Block 3: dynamic content — no cache_control
        assert_eq!(blocks[2]["type"], "text");
        assert_eq!(blocks[2]["text"], "memory section");
        assert!(blocks[2].get("cache_control").is_none());
    }

    #[test]
    fn test_build_cached_system_blocks_no_sentinels() {
        let prompt = "plain system prompt with no markers";
        let blocks = build_cached_system_blocks(prompt);

        // Falls through as a single block with no cache_control (trailing/None hint)
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["text"], "plain system prompt with no markers");
        assert!(blocks[0].get("cache_control").is_none());
    }

    #[test]
    fn test_build_cached_system_blocks_empty_regions_skipped() {
        use crate::runtime::cache::sentinels::{CACHE_SENTINEL_1HR, CACHE_SENTINEL_5M};

        // No user notes or skills — empty middle region
        let prompt = format!(
            "stable content{}{}dynamic memory",
            CACHE_SENTINEL_1HR, CACHE_SENTINEL_5M
        );
        let blocks = build_cached_system_blocks(&prompt);

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["text"], "stable content");
        assert_eq!(blocks[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(blocks[1]["text"], "dynamic memory");
        assert!(blocks[1].get("cache_control").is_none());
    }
}
