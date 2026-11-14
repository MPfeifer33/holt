// providers/anthropic.rs — Anthropic Messages API provider (OAuth path).
//
// The OAuth headers, Claude Code identity headers, tool conversion and cache
// block layout are unchanged from the original runtime/anthropic.rs.
//
// Request and stream shape were brought up to the current Messages API in
// November 2026: models from 4.6 on think adaptively (`thinking.type =
// "adaptive"`, depth via `output_config.effort`) and reject a manual
// `budget_tokens`; 4.5 and earlier keep the manual budget. Thinking blocks
// are kept apart with their signatures and replayed exactly as received.
// These changes are covered by the unit tests below; they have not been
// exercised against the live API from this repository.

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

/// Interleaved thinking is automatic under adaptive thinking; only the
/// manual-budget models still need the header.
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// `max_tokens` covers thinking as well as the reply, so an adaptive
/// thinking request never goes out with less than this.
const THINKING_MAX_TOKENS_FLOOR: u32 = 20_480;

/// Upper bound for an adaptive thinking request. Every adaptive model
/// accepts at least this much output.
const ADAPTIVE_MAX_TOKENS_CEILING: u32 = 64_000;

/// Key under which this provider stores the thinking blocks of a reply in
/// `reasoning_items`, exactly as received, so they can be replayed.
pub(crate) const THINKING_BLOCKS_KEY: &str = "anthropic_blocks";

// =============================================================================
// Thinking: which request shape a model takes
// =============================================================================

/// Claude 4.5 and earlier take a manual thinking budget; everything from
/// 4.6 on is adaptive. An id this does not recognise is assumed current.
pub(crate) fn takes_thinking_budget(model: &str) -> bool {
    let l = model.to_ascii_lowercase();
    l.contains("claude-3")
        || ["-4-5", "-4-1", "-4-0", "-4-2025"]
            .iter()
            .any(|m| l.contains(m))
}

/// The thinking-related parts of a request, decided from the model id and
/// the agent's thinking settings.
#[derive(Debug, PartialEq)]
pub(crate) struct ThinkingRequest {
    pub max_tokens: u32,
    /// The `thinking` field, when one is sent.
    pub thinking: Option<serde_json::Value>,
    /// The `output_config` field, when one is sent.
    pub output_config: Option<serde_json::Value>,
    /// Whether the interleaved-thinking beta header is needed.
    pub interleaved_beta: bool,
}

pub(crate) fn thinking_request(
    model: &str,
    thinking_enabled: bool,
    thinking_budget: u32,
    max_tokens: u32,
) -> ThinkingRequest {
    if takes_thinking_budget(model) {
        // Manual budget: `budget_tokens` is a sub-allocation of `max_tokens`.
        if thinking_enabled {
            ThinkingRequest {
                max_tokens: max_tokens.max(thinking_budget + 4096).min(32_000),
                thinking: Some(serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": thinking_budget,
                })),
                output_config: None,
                interleaved_beta: true,
            }
        } else {
            ThinkingRequest {
                max_tokens: max_tokens.min(16_384),
                thinking: None,
                output_config: None,
                interleaved_beta: false,
            }
        }
    } else if thinking_enabled {
        // Adaptive: the model decides how much to think. `display` asks for
        // summarized thinking text; the default on current models is none.
        ThinkingRequest {
            max_tokens: max_tokens
                .max(THINKING_MAX_TOKENS_FLOOR)
                .min(ADAPTIVE_MAX_TOKENS_CEILING),
            thinking: Some(serde_json::json!({"type": "adaptive", "display": "summarized"})),
            output_config: None,
            interleaved_beta: false,
        }
    } else {
        // Thinking cannot be switched off on the 5 family, so "off" means:
        // say nothing about thinking and ask for low effort.
        ThinkingRequest {
            max_tokens: max_tokens.min(16_384),
            thinking: None,
            output_config: Some(serde_json::json!({"effort": "low"})),
            interleaved_beta: false,
        }
    }
}

/// The thinking blocks of a stored assistant message that may be sent back:
/// only blocks this provider received and stored itself. Thinking text
/// without a signature (older conversations, other providers) is never
/// replayed, because the API rejects an unsigned thinking block.
fn replayable_thinking(msg: &serde_json::Value) -> Vec<serde_json::Value> {
    msg.get("reasoning_items")
        .and_then(|r| r.get(THINKING_BLOCKS_KEY))
        .and_then(|b| b.as_array())
        .cloned()
        .unwrap_or_default()
}

/// Splits a byte stream into lines without decoding a character that is
/// still arriving: bytes are held until their line is complete.
#[derive(Default)]
struct LineBuffer {
    bytes: Vec<u8>,
}

impl LineBuffer {
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.bytes.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(nl) = self.bytes.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.bytes.drain(..=nl).collect();
            lines.push(String::from_utf8_lossy(&line[..nl]).trim().to_string());
        }
        lines
    }
}

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
                let tool_calls = msg.get("tool_calls").and_then(|tc| tc.as_array());

                // Thinking blocks go back one for one, each with its own
                // signature, including empty signed and redacted blocks.
                let mut blocks: Vec<serde_json::Value> = replayable_thinking(msg);

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

        // max_tokens covers ALL output (thinking + response combined). The
        // shape of the thinking request depends on the model generation.
        let thinking_req = thinking_request(
            &self.model,
            self.thinking_enabled,
            self.thinking_budget,
            max_tokens,
        );
        let effective_max_tokens = thinking_req.max_tokens;

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

        if let Some(thinking) = &thinking_req.thinking {
            body["thinking"] = thinking.clone();
        }
        if let Some(output_config) = &thinking_req.output_config {
            body["output_config"] = output_config.clone();
        }

        // Build beta header: OAuth + Claude Code identity. Prompt caching and
        // adaptive interleaved thinking are generally available and need none.
        let mut betas = vec![CLAUDE_CODE_BETA.to_string(), OAUTH_BETA.to_string()];
        if thinking_req.interleaved_beta {
            betas.push(INTERLEAVED_THINKING_BETA.to_string());
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
        let mut lines = LineBuffer::default();
        let mut assembler = StreamAssembler::default();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| StreamError::ApiError(e.to_string()))?;
            for line in lines.push(&chunk) {
                assembler.push_line(&line, on_token, agent_id)?;
            }
        }

        let response = assembler.finish()?;

        // Emit Done event (required by frontend to finalize message display)
        on_token(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Done,
            data: serde_json::json!({}),
        });

        Ok(response)
    }
}

// =============================================================================
// Stream assembly
// =============================================================================

/// One content block as it arrives.
#[derive(Default)]
struct Block {
    kind: String,
    text: String,
    signature: String,
    /// `redacted_thinking` payload.
    data: String,
    tool_id: String,
    tool_name: String,
    tool_input: String,
}

/// Turns the SSE lines of one reply into an `AssembledResponse`.
#[derive(Default)]
struct StreamAssembler {
    current: Option<Block>,
    text_content: String,
    thinking_content: String,
    /// Thinking blocks exactly as received, for replay.
    thinking_blocks: Vec<serde_json::Value>,
    tool_calls: Vec<ToolCallRecord>,
    usage: Option<UsageInfo>,
    stop_reason: Option<String>,
    /// `stop_details.category` when the stop was a refusal.
    refusal_category: Option<String>,
}

impl StreamAssembler {
    fn push_line(
        &mut self,
        line: &str,
        on_token: &(dyn Fn(StreamEvent) + Send + Sync),
        agent_id: &str,
    ) -> Result<(), StreamError> {
        if line.is_empty() || line.starts_with(':') {
            return Ok(());
        }
        let Some(data) = line.strip_prefix("data:").map(str::trim_start) else {
            return Ok(());
        };
        if data == "[DONE]" {
            return Ok(());
        }
        let event: serde_json::Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };

        match event.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "content_block_start" => {
                let block = event.get("content_block");
                let field = |name: &str| {
                    block
                        .and_then(|b| b.get(name))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                };
                self.current = Some(Block {
                    kind: field("type"),
                    data: field("data"),
                    tool_id: field("id"),
                    tool_name: field("name"),
                    ..Default::default()
                });
            }
            "content_block_delta" => {
                let (Some(delta), Some(block)) = (event.get("delta"), self.current.as_mut()) else {
                    return Ok(());
                };
                let piece = |name: &str| delta.get(name).and_then(|t| t.as_str());
                match delta.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "text_delta" => {
                        if let Some(text) = piece("text") {
                            self.text_content.push_str(text);
                            on_token(StreamEvent {
                                agent_id: agent_id.to_string(),
                                event_type: StreamEventType::Token,
                                data: serde_json::json!({"text": text}),
                            });
                        }
                    }
                    "thinking_delta" => {
                        if let Some(thinking) = piece("thinking") {
                            block.text.push_str(thinking);
                            self.thinking_content.push_str(thinking);
                            on_token(StreamEvent {
                                agent_id: agent_id.to_string(),
                                event_type: StreamEventType::Thinking,
                                data: serde_json::json!({"text": thinking}),
                            });
                        }
                    }
                    "signature_delta" => {
                        if let Some(signature) = piece("signature") {
                            block.signature.push_str(signature);
                        }
                    }
                    "input_json_delta" => {
                        if let Some(partial) = piece("partial_json") {
                            block.tool_input.push_str(partial);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let Some(block) = self.current.take() else {
                    return Ok(());
                };
                match block.kind.as_str() {
                    "tool_use" => {
                        let arguments: serde_json::Value = serde_json::from_str(&block.tool_input)
                            .unwrap_or(serde_json::json!({}));
                        self.tool_calls.push(ToolCallRecord {
                            id: block.tool_id,
                            name: block.tool_name,
                            arguments,
                        });
                    }
                    // A signed block goes back unchanged even when its text
                    // is empty; an unsigned one cannot be replayed at all.
                    "thinking" if !block.signature.is_empty() => {
                        self.thinking_blocks.push(serde_json::json!({
                            "type": "thinking",
                            "thinking": block.text,
                            "signature": block.signature,
                        }));
                    }
                    "redacted_thinking" => {
                        self.thinking_blocks.push(serde_json::json!({
                            "type": "redacted_thinking",
                            "data": block.data,
                        }));
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(reason) = event.pointer("/delta/stop_reason").and_then(|s| s.as_str()) {
                    self.stop_reason = Some(reason.to_string());
                }
                if let Some(category) = event
                    .pointer("/delta/stop_details/category")
                    .and_then(|c| c.as_str())
                {
                    self.refusal_category = Some(category.to_string());
                }
                if let Some(usage) = event.get("usage") {
                    let tokens =
                        |name: &str| usage.get(name).and_then(|t| t.as_u64()).map(|t| t as u32);
                    let prompt_tokens = tokens("input_tokens")
                        .or(self.usage.as_ref().and_then(|u| u.prompt_tokens));
                    self.usage = Some(UsageInfo {
                        prompt_tokens,
                        completion_tokens: tokens("output_tokens"),
                        total_tokens: None,
                    });
                }
            }
            "message_start" => {
                let input = event
                    .pointer("/message/usage/input_tokens")
                    .and_then(|t| t.as_u64())
                    .map(|t| t as u32);
                if self.usage.is_none() && input.is_some() {
                    self.usage = Some(UsageInfo {
                        prompt_tokens: input,
                        completion_tokens: None,
                        total_tokens: None,
                    });
                }
            }
            "error" => {
                let error_msg = event
                    .pointer("/error/message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown Anthropic API error");
                return Err(StreamError::ApiError(format!(
                    "Anthropic error: {}",
                    error_msg
                )));
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(self) -> Result<AssembledResponse, StreamError> {
        if self.stop_reason.as_deref() == Some("refusal") {
            // A safety classifier stopped the request. There is nothing to
            // continue from, so it surfaces as an error, not an empty reply.
            let category = self.refusal_category.as_deref().unwrap_or("unspecified");
            return Err(StreamError::ApiError(format!(
                "the model declined this request (safety classifier, category: {category})"
            )));
        }
        let reasoning_items = if self.thinking_blocks.is_empty() {
            None
        } else {
            Some(serde_json::json!({ THINKING_BLOCKS_KEY: self.thinking_blocks }))
        };
        Ok(AssembledResponse {
            content: self.text_content,
            tool_calls: self.tool_calls,
            usage: self.usage,
            finish_reason: self.stop_reason,
            thinking_content: if self.thinking_content.is_empty() {
                None
            } else {
                Some(self.thinking_content)
            },
            reasoning_items,
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
    fn test_signed_thinking_blocks_replay_one_for_one() {
        let messages = vec![serde_json::json!({
            "role": "assistant",
            "content": "The answer is 42.",
            "thinking_content": "firstsecond",
            "reasoning_items": { THINKING_BLOCKS_KEY: [
                {"type": "thinking", "thinking": "first", "signature": "SIG1"},
                {"type": "thinking", "thinking": "", "signature": "SIG2"},
                {"type": "redacted_thinking", "data": "OPAQUE"},
            ]}
        })];
        let (_, msgs) = convert_messages_to_anthropic(&messages);
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 4);
        assert_eq!(content[0]["signature"], "SIG1");
        assert_eq!(
            content[1]["thinking"], "",
            "an empty signed block goes back unchanged"
        );
        assert_eq!(content[1]["signature"], "SIG2");
        assert_eq!(content[2]["type"], "redacted_thinking");
        assert_eq!(content[3]["type"], "text");
    }

    #[test]
    fn test_unsigned_or_foreign_thinking_is_never_replayed() {
        let messages = vec![
            // An older conversation: thinking text, no stored blocks.
            serde_json::json!({
                "role": "assistant",
                "content": "The answer is 42.",
                "thinking_content": "Let me reason through this..."
            }),
            serde_json::json!({"role": "user", "content": "and?"}),
            // Reasoning items written by another provider.
            serde_json::json!({
                "role": "assistant",
                "content": "Still 42.",
                "reasoning_items": [{"type": "reasoning", "encrypted_content": "xyz"}]
            }),
        ];
        let (_, msgs) = convert_messages_to_anthropic(&messages);
        for m in [&msgs[0], &msgs[2]] {
            let content = m["content"].as_array().unwrap();
            assert_eq!(content.len(), 1);
            assert_eq!(content[0]["type"], "text");
        }
    }

    #[test]
    fn test_adaptive_models_never_get_a_budget() {
        for model in [
            "claude-opus-5-5",
            "claude-opus-4-6",
            "claude-sonnet-5",
            "claude-fable-5-1",
            "claude-haiku-5-5",
            "claude-some-future-model",
        ] {
            let on = thinking_request(model, true, 16_384, 4096);
            assert_eq!(on.thinking.as_ref().unwrap()["type"], "adaptive", "{model}");
            assert!(on.thinking.as_ref().unwrap().get("budget_tokens").is_none());
            assert_eq!(on.max_tokens, THINKING_MAX_TOKENS_FLOOR, "{model}");
            assert!(!on.interleaved_beta);

            let off = thinking_request(model, false, 16_384, 4096);
            assert!(off.thinking.is_none());
            assert_eq!(off.output_config.as_ref().unwrap()["effort"], "low");
            assert_eq!(off.max_tokens, 4096);
        }
        let big = thinking_request("claude-opus-5-5", true, 16_384, 200_000);
        assert_eq!(big.max_tokens, ADAPTIVE_MAX_TOKENS_CEILING);
    }

    #[test]
    fn test_budget_models_keep_the_manual_shape() {
        for model in [
            "claude-haiku-4-5",
            "claude-haiku-4-5-20251001",
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4-20250514",
            "claude-3-7-sonnet-20250219",
        ] {
            let on = thinking_request(model, true, 10_000, 4096);
            let thinking = on.thinking.as_ref().unwrap();
            assert_eq!(thinking["type"], "enabled", "{model}");
            assert_eq!(thinking["budget_tokens"], 10_000);
            assert_eq!(on.max_tokens, 14_096);
            assert!(
                on.output_config.is_none(),
                "{model} has no effort parameter"
            );
            assert!(on.interleaved_beta);

            let off = thinking_request(model, false, 10_000, 4096);
            assert!(off.thinking.is_none() && off.output_config.is_none());
        }
    }

    fn assemble(lines: &[&str]) -> Result<AssembledResponse, StreamError> {
        let mut assembler = StreamAssembler::default();
        for line in lines {
            assembler.push_line(line, &|_| {}, "demo")?;
        }
        assembler.finish()
    }

    #[test]
    fn test_stream_keeps_thinking_blocks_apart_with_their_signatures() {
        let r = assemble(&[
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":12}}}"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"first"}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"SIG1"}}"#,
            r#"data: {"type":"content_block_stop","index":0}"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"signature_delta","signature":"SIG2"}}"#,
            r#"data: {"type":"content_block_stop","index":1}"#,
            r#"data: {"type":"content_block_start","index":2,"content_block":{"type":"redacted_thinking","data":"OPAQUE"}}"#,
            r#"data: {"type":"content_block_stop","index":2}"#,
            r#"data: {"type":"content_block_start","index":3,"content_block":{"type":"text","text":""}}"#,
            r#"data: {"type":"content_block_delta","index":3,"delta":{"type":"text_delta","text":"hello"}}"#,
            r#"data: {"type":"content_block_stop","index":3}"#,
            r#"data: {"type":"content_block_start","index":4,"content_block":{"type":"tool_use","id":"tu_1","name":"read_file"}}"#,
            r#"data: {"type":"content_block_delta","index":4,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
            r#"data: {"type":"content_block_delta","index":4,"delta":{"type":"input_json_delta","partial_json":"\"a.txt\"}"}}"#,
            r#"data: {"type":"content_block_stop","index":4}"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":30}}"#,
            r#"data: {"type":"message_stop"}"#,
        ])
        .unwrap();

        assert_eq!(r.content, "hello");
        assert_eq!(r.thinking_content.as_deref(), Some("first"));
        assert_eq!(r.finish_reason.as_deref(), Some("tool_use"));
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].arguments["path"], "a.txt");
        let usage = r.usage.unwrap();
        assert_eq!(
            usage.prompt_tokens,
            Some(12),
            "input tokens survive the final usage event"
        );
        assert_eq!(usage.completion_tokens, Some(30));
        assert_eq!(
            r.reasoning_items.unwrap()[THINKING_BLOCKS_KEY],
            serde_json::json!([
                {"type": "thinking", "thinking": "first", "signature": "SIG1"},
                {"type": "thinking", "thinking": "", "signature": "SIG2"},
                {"type": "redacted_thinking", "data": "OPAQUE"},
            ])
        );
    }

    #[test]
    fn test_a_refusal_is_an_error_not_an_empty_reply() {
        let err = assemble(&[
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":5}}}"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"category":"cyber"}},"usage":{"output_tokens":0}}"#,
        ])
        .unwrap_err();
        assert!(err.to_string().contains("declined"), "{err}");
        assert!(err.to_string().contains("cyber"), "{err}");
    }

    #[test]
    fn test_stream_error_event_surfaces() {
        let err =
            assemble(&[r#"data: {"type":"error","error":{"message":"overloaded"}}"#]).unwrap_err();
        assert!(err.to_string().contains("overloaded"));
    }

    #[test]
    fn test_line_buffer_holds_a_split_character_until_its_line_is_complete() {
        let text = "data: caf\u{e9} \u{1f98a}\n";
        let bytes = text.as_bytes();
        // Split inside the four-byte character.
        let cut = bytes.len() - 3;
        let mut lines = LineBuffer::default();
        assert!(lines.push(&bytes[..cut]).is_empty());
        let out = lines.push(&bytes[cut..]);
        assert_eq!(out, vec!["data: caf\u{e9} \u{1f98a}".to_string()]);
    }

    #[test]
    fn test_anthropic_provider_construction() {
        let provider = AnthropicProvider::new("test-bearer-token", "claude-opus-5-5", true, 10000);
        assert_eq!(provider.name(), "Anthropic Messages");
        assert_eq!(provider.model, "claude-opus-5-5");
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
