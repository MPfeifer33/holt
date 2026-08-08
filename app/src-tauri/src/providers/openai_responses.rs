// app/src-tauri/src/providers/openai_responses.rs
//
// OpenAI Responses API provider for the GPT-5 family and o-series reasoning
// models. Routed exclusively for `api.openai.com` endpoints via
// `providers::mod::detect_and_build()`. All other OpenAI-compatible endpoints
// (Grok, Ollama, llama.cpp, LM Studio, Groq, Together, etc.) continue to use
// the hand-rolled Chat Completions `OpenAIProvider`.
//
// Why this exists:
//
//   1. GPT-5.4 with `reasoning: none` does not support tool calling in Chat
//      Completions. Per OpenAI's own docs: "Starting with GPT-5.4, tool calling
//      is not supported in Chat Completions with `reasoning: none`." For any
//      agent platform that wants tool-calling GPT-5.4, the Responses API is
//      the only path.
//
//   2. Chat Completions requires strict `assistant.tool_calls` → `tool`
//      adjacency in the messages array, correlated by position. Holt's
//      engine has repeatedly tripped this constraint (context-builder pinning,
//      turn-limit System message interposition). The Responses API correlates
//      tool calls to tool outputs by `call_id`, not by position — the
//      adjacency requirement architecturally does not exist.
//
//   3. The Responses API reports 40–80% lower cost for agent workloads due to
//      improved prompt caching on OpenAI's side.
//
// Implementation notes:
//
// - Uses the `async-openai` crate (version 0.34 with the `responses` feature
//   flag explicitly enabled). Hand-rolling serde for the Responses API would
//   be ~5,000 lines of types; the crate provides typed bindings and SSE event
//   dispatch for free.
//
// - The `CompletionProvider` trait receives messages as `&[serde_json::Value]`
//   already converted to Chat Completions shape by `context::message_to_json()`.
//   This provider re-parses those values into Responses `InputItem`s. The
//   `responses_phase` and `reasoning_items` extra fields set by context.rs are
//   read out during conversion to preserve phase tags and chain-of-thought
//   across turns.
//
// - The `client: &reqwest::Client` parameter passed by the trait is IGNORED
//   because `async-openai` owns its own internal HTTP client. This is a v1
//   inefficiency (two connection pools for OpenAI agents) that could be
//   addressed in v2 by sharing the engine's pooled client.
//
// - Conditional parameter gating: when `reasoning_effort != None`, the
//   Responses API rejects `temperature`, `top_p`, and `logprobs`. The request
//   builder omits those fields via `ReasoningEffort::allows_sampling_params()`.
//
// v1 scope: function tools, streaming, reasoning + verbosity, phase inference.
// v2 deferred: built-in OpenAI tools, tool_search, allowed_tools, reasoning
// item round-trip, previous_response_id chaining, shared reqwest client.

use async_openai::config::OpenAIConfig;
use async_openai::error::OpenAIError;
use async_openai::types::responses::{
    CreateResponseArgs, EasyInputContent, EasyInputMessage, FunctionCallOutput,
    FunctionCallOutputItemParam, FunctionTool, FunctionToolCall, InputItem, Item, MessagePhase,
    MessageType, OutputItem, Reasoning, ReasoningEffort, ResponseStreamEvent, ResponseTextParam,
    ResponseUsage, Role, TextResponseFormatConfiguration, Tool, Verbosity,
};
use async_openai::Client;
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashMap;

use crate::runtime::agent::ToolCallRecord;
use crate::runtime::openai_responses_settings::{
    OpenAIResponsesSettings, ReasoningEffort as HoltEffort, Verbosity as HoltVerbosity,
};

use super::types::{
    AssembledResponse, CompletionProvider, StreamError, StreamEvent, StreamEventType, UsageInfo,
};

// ===========================================================================
// Provider struct + constructor
// ===========================================================================

/// Provider for OpenAI's Responses API (GPT-5 family, `api.openai.com` only).
#[derive(Debug)]
pub struct OpenAIResponsesProvider {
    /// Base endpoint URL. Typically `https://api.openai.com/v1`, but the
    /// provider respects whatever the user configured so it can be pointed at
    /// test fixtures if needed.
    pub(crate) endpoint: String,
    /// Bearer token for api.openai.com. If None, `send_completion()` returns
    /// `StreamError::ApiError` at request time.
    pub(crate) api_key: Option<String>,
    /// Per-agent Responses configuration: model, reasoning effort, verbosity,
    /// max_output_tokens, temperature, top_p. Pulled from
    /// `AgentSlot.openai_responses_settings` at provider-construction time.
    pub(crate) settings: OpenAIResponsesSettings,
}

impl OpenAIResponsesProvider {
    pub fn new(endpoint: &str, api_key: Option<&str>, settings: OpenAIResponsesSettings) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            api_key: api_key.map(|s| s.to_string()),
            settings,
        }
    }

    /// Build an `async-openai` client configured for this provider's endpoint
    /// and API key. Fails with `StreamError::ApiError` if no key is present.
    fn build_client(&self) -> Result<Client<OpenAIConfig>, StreamError> {
        let key = self.api_key.as_deref().ok_or_else(|| {
            StreamError::ApiError("OpenAI Responses provider requires an API key".to_string())
        })?;

        let config = OpenAIConfig::new()
            .with_api_key(key)
            .with_api_base(&self.endpoint);
        Ok(Client::with_config(config))
    }
}

// ===========================================================================
// CompletionProvider impl
// ===========================================================================

#[async_trait]
impl CompletionProvider for OpenAIResponsesProvider {
    fn name(&self) -> &'static str {
        "OpenAI Responses"
    }

    async fn send_completion(
        &self,
        _client: &reqwest::Client,
        messages: &[Value],
        tools: Option<&[Value]>,
        max_tokens: u32,
        on_token: &(dyn Fn(StreamEvent) + Send + Sync),
        agent_id: &str,
    ) -> Result<AssembledResponse, StreamError> {
        // 1. Split messages into `instructions` (system role) and `input` items.
        let (instructions, input_items) = convert_messages(messages);

        // 2. Convert tools if any are provided.
        let openai_tools = match tools {
            Some(ts) if !ts.is_empty() => Some(convert_tools(ts)?),
            _ => None,
        };

        // 3. Build the request using async-openai's derive_builder API.
        let client = self.build_client()?;

        // Settings.max_output_tokens takes precedence over engine's max_tokens.
        let effective_max = self.settings.max_output_tokens.unwrap_or(max_tokens);
        let effort = map_effort(self.settings.reasoning_effort);
        let verbosity = map_verbosity(self.settings.verbosity);
        let allows_sampling = self.settings.reasoning_effort.allows_sampling_params();

        let mut builder = CreateResponseArgs::default();
        builder
            .model(self.settings.model.clone())
            .input(input_items)
            .max_output_tokens(effective_max)
            .parallel_tool_calls(true)
            .stream(true)
            .reasoning(Reasoning {
                effort: Some(effort),
                summary: None,
            })
            .text(ResponseTextParam {
                format: TextResponseFormatConfiguration::Text,
                verbosity: Some(verbosity),
            });

        if !instructions.is_empty() {
            builder.instructions(instructions);
        }

        if let Some(ts) = openai_tools {
            builder.tools(ts);
        }

        // Conditional parameter gating: temperature/top_p only valid when
        // reasoning.effort == "none". See spec § 11.
        if allows_sampling {
            if let Some(temp) = self.settings.temperature {
                builder.temperature(temp);
            }
            if let Some(top_p) = self.settings.top_p {
                builder.top_p(top_p);
            }
        }

        let request = builder
            .build()
            .map_err(|e| StreamError::ApiError(format!("Responses request builder failed: {e}")))?;

        // 4. Send the streaming request and iterate events.
        let mut stream = client
            .responses()
            .create_stream(request)
            .await
            .map_err(map_openai_error)?;

        let mut state = StreamAccumulator::default();

        while let Some(event) = stream.next().await {
            let event = event.map_err(map_openai_error)?;
            handle_event(event, &mut state, agent_id, on_token)?;
            if state.terminated {
                break;
            }
        }

        // Emit Done event so the frontend knows this streaming response is
        // finished. ChatTile and other listeners use Done as the signal to
        // flush their per-stream accumulator buffers (`streamingContent`),
        // mark "agent is responding" indicators as cleared, and finalize
        // any in-progress UI state.
        //
        // Without this, the chat tile keeps the partial-stream buffer
        // around indefinitely. When the engine then emits
        // `ConversationUpdated` (after run_agentic_loop adds the final
        // assistant message to the conversation and re-fetches it from
        // the backend), the re-fetched message is shown ALONGSIDE the
        // never-flushed streaming buffer — producing duplicate display
        // of the same response, plus a "still streaming" indicator that
        // never goes away.
        //
        // All other providers emit this event:
        //   - openai.rs:322
        //   - anthropic.rs:584
        //   - xai.rs:175
        //   - gemini.rs:475
        //   - agent_sdk.rs:504
        // The Responses provider was the lone exception, which is why
        // this bug only manifested for GPT-5.4 agents on the new path.
        on_token(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Done,
            data: serde_json::json!({}),
        });

        Ok(state.finalize())
    }
}

// ===========================================================================
// Message conversion: Chat-Completions-shape JSON → Responses InputItems
// ===========================================================================

/// Convert the engine's Chat-Completions-shaped message array into a
/// Responses API `(instructions, Vec<InputItem>)` pair.
///
/// System messages are pulled out of the input stream and concatenated into
/// the `instructions` string (joined with `\n\n` if multiple). Every other
/// message is converted to an appropriate `InputItem` variant. Extra fields
/// `responses_phase` and `reasoning_items` set by `context::message_to_json()`
/// are read out to preserve phase tags and chain-of-thought across turns.
fn convert_messages(messages: &[Value]) -> (String, Vec<InputItem>) {
    let mut instructions = String::new();
    let mut items: Vec<InputItem> = Vec::new();

    for msg in messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");

        match role {
            "system" => {
                // Accumulate system content into instructions.
                if let Some(content) = extract_text_content(msg) {
                    if !instructions.is_empty() {
                        instructions.push_str("\n\n");
                    }
                    instructions.push_str(&content);
                }
            }
            "user" => {
                let content = extract_text_content(msg).unwrap_or_default();
                items.push(InputItem::EasyMessage(EasyInputMessage {
                    r#type: MessageType::Message,
                    role: Role::User,
                    content: EasyInputContent::Text(content),
                    phase: None,
                }));
            }
            "assistant" => {
                let text = extract_text_content(msg).unwrap_or_default();
                let has_tool_calls = msg
                    .get("tool_calls")
                    .and_then(|v| v.as_array())
                    .is_some_and(|a| !a.is_empty());
                let explicit_phase = parse_phase(msg);

                // If the assistant message has text content, emit a message
                // Item with the phase tag. Phase is either the explicit value
                // from `responses_phase` extras, or inferred: Commentary when
                // accompanied by tool calls (it's a preamble), FinalAnswer
                // otherwise (it's the completed response).
                if !text.is_empty() {
                    let phase = explicit_phase.unwrap_or(if has_tool_calls {
                        MessagePhase::Commentary
                    } else {
                        MessagePhase::FinalAnswer
                    });
                    items.push(InputItem::EasyMessage(EasyInputMessage {
                        r#type: MessageType::Message,
                        role: Role::Assistant,
                        content: EasyInputContent::Text(text),
                        phase: Some(phase),
                    }));
                }

                // Emit function_call Items for each tool_call in the message.
                if let Some(tcs) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in tcs {
                        let call_id = tc
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let func = tc.get("function");
                        let name = func
                            .and_then(|f| f.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments = func
                            .and_then(|f| f.get("arguments"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}")
                            .to_string();

                        items.push(InputItem::Item(Item::FunctionCall(FunctionToolCall {
                            arguments,
                            call_id,
                            name,
                            namespace: None,
                            id: None,
                            status: None,
                        })));
                    }
                }

                // v1: reasoning_items are NOT round-tripped. The extra field
                // is read on ingress (stored from streaming) but not replayed
                // here. Re-reasoning each turn is a v1 cost we accept. See
                // spec § 12.2 for the v2 upgrade plan.
                //
                // Field preserved in the extras for future use:
                //   let _ = msg.get("reasoning_items");
            }
            "tool" => {
                let call_id = msg
                    .get("tool_call_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let output_text = extract_text_content(msg).unwrap_or_default();
                items.push(InputItem::Item(Item::FunctionCallOutput(
                    FunctionCallOutputItemParam {
                        call_id,
                        output: FunctionCallOutput::Text(output_text),
                        id: None,
                        status: None,
                    },
                )));
            }
            other => {
                tracing::warn!(
                    role = other,
                    "OpenAIResponsesProvider: unknown message role, skipping"
                );
            }
        }
    }

    (instructions, items)
}

/// Extract the `content` field of a message as a plain string. Handles both
/// the plain-string shape (`{content: "hello"}`) and the content-array shape
/// (`{content: [{type: "text", text: "..."}]}`) that `context::message_to_json`
/// may produce. Non-text parts are ignored in v1 (image content deferred).
fn extract_text_content(msg: &Value) -> Option<String> {
    let content = msg.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = content.as_array() {
        let mut out = String::new();
        for part in arr {
            if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                out.push_str(t);
            }
        }
        if out.is_empty() {
            return None;
        }
        return Some(out);
    }
    None
}

/// Parse the `responses_phase` extra field from an assistant message.
/// Returns `None` if the field is missing or doesn't match a known value.
fn parse_phase(msg: &Value) -> Option<MessagePhase> {
    let phase = msg.get("responses_phase")?.as_str()?;
    match phase {
        "commentary" => Some(MessagePhase::Commentary),
        "final_answer" => Some(MessagePhase::FinalAnswer),
        _ => None,
    }
}

// ===========================================================================
// Tool schema conversion: Chat-Completions tool JSON → Responses Tool enum
// ===========================================================================

/// Convert the engine's tool definitions (in Chat Completions wrapped shape
/// `{ type: "function", function: { name, description, parameters } }`) into
/// Responses API `Tool::Function(FunctionTool)` instances.
///
/// Accepts both the wrapped shape (nested under `function`) and the flatter
/// shape (fields at top level) so this function works regardless of whether
/// the caller produced Chat-Completions-compat JSON or Responses-native JSON.
fn convert_tools(tools: &[Value]) -> Result<Vec<Tool>, StreamError> {
    let mut out = Vec::with_capacity(tools.len());
    for t in tools {
        let ttype = t.get("type").and_then(|v| v.as_str()).unwrap_or("function");
        if ttype != "function" {
            tracing::warn!(
                tool_type = ttype,
                "Skipping non-function tool for Responses API"
            );
            continue;
        }

        // Accept wrapped or flat shape.
        let func = t.get("function").unwrap_or(t);
        let name = func
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| StreamError::ApiError("tool missing name".to_string()))?
            .to_string();
        let description = func
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let parameters = func.get("parameters").cloned();

        out.push(Tool::Function(FunctionTool {
            name,
            description,
            parameters,
            strict: Some(false),
            defer_loading: None,
        }));
    }
    Ok(out)
}

// ===========================================================================
// Settings mapping: Holt enums → async-openai enums
// ===========================================================================

fn map_effort(e: HoltEffort) -> ReasoningEffort {
    match e {
        HoltEffort::None => ReasoningEffort::None,
        HoltEffort::Minimal => ReasoningEffort::Minimal,
        HoltEffort::Low => ReasoningEffort::Low,
        HoltEffort::Medium => ReasoningEffort::Medium,
        HoltEffort::High => ReasoningEffort::High,
        HoltEffort::Xhigh => ReasoningEffort::Xhigh,
    }
}

fn map_verbosity(v: HoltVerbosity) -> Verbosity {
    match v {
        HoltVerbosity::Low => Verbosity::Low,
        HoltVerbosity::Medium => Verbosity::Medium,
        HoltVerbosity::High => Verbosity::High,
    }
}

// ===========================================================================
// Streaming: accumulator + event handler
// ===========================================================================

/// In-progress tool call being assembled from streaming events. Keyed by
/// `item_id` in the accumulator's `HashMap` so multiple parallel tool calls
/// can be tracked simultaneously.
struct PendingToolCall {
    call_id: String,
    name: String,
    arguments: String,
}

/// Accumulates streaming state and produces the final `AssembledResponse`.
#[derive(Default)]
struct StreamAccumulator {
    content: String,
    thinking: String,
    /// Tool calls being assembled, keyed by `item_id` from
    /// `ResponseOutputItemAdded`. Arguments are appended by
    /// `ResponseFunctionCallArgumentsDelta` events.
    pending_tool_calls: HashMap<String, PendingToolCall>,
    /// Finalized tool calls in the order they were completed.
    tool_calls: Vec<ToolCallRecord>,
    usage: Option<UsageInfo>,
    finish_reason: Option<String>,
    /// True once a terminal event (Completed, Failed, Incomplete, or Error)
    /// has been observed. The stream loop breaks when this is set.
    terminated: bool,
}

impl StreamAccumulator {
    fn finalize(self) -> AssembledResponse {
        AssembledResponse {
            content: self.content,
            tool_calls: self.tool_calls,
            usage: self.usage,
            finish_reason: self.finish_reason,
            thinking_content: if self.thinking.is_empty() {
                None
            } else {
                Some(self.thinking)
            },
        }
    }
}

/// Dispatch a single streaming event from `ResponseStreamEvent` to the
/// appropriate accumulator update and emit user-visible `StreamEvent`s via
/// the `on_token` callback.
fn handle_event(
    event: ResponseStreamEvent,
    state: &mut StreamAccumulator,
    agent_id: &str,
    on_token: &(dyn Fn(StreamEvent) + Send + Sync),
) -> Result<(), StreamError> {
    use ResponseStreamEvent::*;
    match event {
        // -- text content streaming --
        ResponseOutputTextDelta(e) => {
            state.content.push_str(&e.delta);
            on_token(StreamEvent {
                agent_id: agent_id.to_string(),
                event_type: StreamEventType::Token,
                data: serde_json::json!({"text": e.delta}),
            });
        }
        ResponseOutputTextDone(_) => {
            // No-op; content already accumulated from deltas.
        }

        // -- reasoning / chain-of-thought streaming --
        ResponseReasoningTextDelta(e) => {
            state.thinking.push_str(&e.delta);
            on_token(StreamEvent {
                agent_id: agent_id.to_string(),
                event_type: StreamEventType::Thinking,
                data: serde_json::json!({"text": e.delta}),
            });
        }
        ResponseReasoningSummaryTextDelta(e) => {
            state.thinking.push_str(&e.delta);
            on_token(StreamEvent {
                agent_id: agent_id.to_string(),
                event_type: StreamEventType::Thinking,
                data: serde_json::json!({"text": e.delta}),
            });
        }

        // -- output item boundaries (function calls start + finalize here) --
        ResponseOutputItemAdded(e) => {
            if let OutputItem::FunctionCall(fc) = &e.item {
                // Stash name + call_id; arguments will stream in via
                // ResponseFunctionCallArgumentsDelta events keyed by item_id.
                let item_id = fc.id.clone().unwrap_or_else(|| fc.call_id.clone());
                state.pending_tool_calls.insert(
                    item_id,
                    PendingToolCall {
                        call_id: fc.call_id.clone(),
                        name: fc.name.clone(),
                        arguments: String::new(),
                    },
                );
            }
        }
        ResponseOutputItemDone(e) => {
            if let OutputItem::FunctionCall(fc) = &e.item {
                // Finalize the pending entry. Prefer `fc.arguments` if present
                // (canonical complete string); otherwise fall back to the
                // accumulated arguments from delta events.
                let item_id = fc.id.clone().unwrap_or_else(|| fc.call_id.clone());
                let pending = state.pending_tool_calls.remove(&item_id);
                let final_args = if !fc.arguments.is_empty() {
                    fc.arguments.clone()
                } else {
                    pending
                        .as_ref()
                        .map(|p| p.arguments.clone())
                        .unwrap_or_default()
                };
                let name = pending
                    .as_ref()
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| fc.name.clone());
                let call_id = pending
                    .as_ref()
                    .map(|p| p.call_id.clone())
                    .unwrap_or_else(|| fc.call_id.clone());

                // Parse arguments string as JSON for our internal
                // ToolCallRecord shape. If parsing fails (unlikely — the
                // model is supposed to produce valid JSON), wrap as a
                // string value so the caller can still see what was sent.
                let arguments: Value = serde_json::from_str(&final_args)
                    .unwrap_or_else(|_| Value::String(final_args.clone()));

                state.tool_calls.push(ToolCallRecord {
                    id: call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                });

                on_token(StreamEvent {
                    agent_id: agent_id.to_string(),
                    event_type: StreamEventType::ToolCall,
                    data: serde_json::json!({
                        "tool": name,
                        "arguments": arguments.to_string(),
                        "result": "",
                    }),
                });
            }
        }

        // -- function call argument streaming (incremental) --
        ResponseFunctionCallArgumentsDelta(e) => {
            if let Some(pending) = state.pending_tool_calls.get_mut(&e.item_id) {
                pending.arguments.push_str(&e.delta);
            }
        }
        ResponseFunctionCallArgumentsDone(_) => {
            // Handled via ResponseOutputItemDone which arrives after this and
            // carries the final canonical FunctionToolCall.
        }

        // -- refusals (ignored in v1, logged for debugging per spec § 8.1) --
        ResponseRefusalDelta(e) => {
            tracing::debug!(
                agent_id = %agent_id,
                delta = %e.delta,
                "OpenAI Responses refusal delta (ignored in v1)"
            );
        }
        ResponseRefusalDone(e) => {
            tracing::debug!(
                agent_id = %agent_id,
                refusal = %e.refusal,
                "OpenAI Responses refusal finalized (ignored in v1)"
            );
        }

        // -- terminal events --
        ResponseCompleted(e) => {
            if let Some(usage) = &e.response.usage {
                state.usage = Some(map_usage(usage));
            }
            state.finish_reason = Some("stop".to_string());
            state.terminated = true;
        }
        ResponseFailed(e) => {
            let err_msg = e
                .response
                .error
                .as_ref()
                .map(|err| err.message.clone())
                .unwrap_or_else(|| "response failed".to_string());
            state.terminated = true;
            return Err(StreamError::ApiError(err_msg));
        }
        ResponseIncomplete(e) => {
            if let Some(usage) = &e.response.usage {
                state.usage = Some(map_usage(usage));
            }
            state.finish_reason = Some("incomplete".to_string());
            state.terminated = true;
        }
        ResponseError(e) => {
            state.terminated = true;
            return Err(StreamError::ApiError(format!(
                "{}: {}",
                e.code.as_deref().unwrap_or("error"),
                e.message
            )));
        }

        // -- ignored lifecycle / noise events --
        _ => {}
    }
    Ok(())
}

/// Map `ResponseUsage` (async-openai) → `UsageInfo` (Holt).
fn map_usage(u: &ResponseUsage) -> UsageInfo {
    UsageInfo {
        prompt_tokens: Some(u.input_tokens),
        completion_tokens: Some(u.output_tokens),
        total_tokens: Some(u.total_tokens),
    }
}

/// Map `OpenAIError` → `StreamError`.
///
/// Collapses all error variants to `StreamError::ApiError` with a descriptive
/// string. v1 does not distinguish rate-limit/5xx/deserialization errors at
/// the StreamError level — the engine loop handles retries at a higher layer.
fn map_openai_error(e: OpenAIError) -> StreamError {
    match e {
        OpenAIError::Reqwest(err) => StreamError::ApiError(err.to_string()),
        OpenAIError::ApiError(api_err) => StreamError::ApiError(api_err.to_string()),
        OpenAIError::JSONDeserialize(err, content) => StreamError::ApiError(format!(
            "JSON deserialize error: {err} (content: {content})"
        )),
        OpenAIError::FileSaveError(msg) | OpenAIError::FileReadError(msg) => {
            StreamError::ApiError(msg)
        }
        OpenAIError::StreamError(err) => StreamError::ApiError(err.to_string()),
        OpenAIError::InvalidArgument(msg) => StreamError::ApiError(msg),
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::openai_responses_settings::{
        OpenAIResponsesSettings, ReasoningEffort as E, Verbosity as V,
    };

    // ── Message conversion ────────────────────────────────────────────────

    #[test]
    fn test_convert_user_message() {
        let msgs = vec![serde_json::json!({
            "role": "user",
            "content": "Hello!"
        })];
        let (instructions, items) = convert_messages(&msgs);
        assert_eq!(instructions, "");
        assert_eq!(items.len(), 1);
        match &items[0] {
            InputItem::EasyMessage(e) => {
                assert_eq!(e.role, Role::User);
                match &e.content {
                    EasyInputContent::Text(t) => assert_eq!(t, "Hello!"),
                    _ => panic!("expected text content"),
                }
            }
            other => panic!("expected EasyMessage, got {other:?}"),
        }
    }

    #[test]
    fn test_system_message_becomes_instructions() {
        let msgs = vec![
            serde_json::json!({"role": "system", "content": "You are a helper."}),
            serde_json::json!({"role": "user", "content": "Hi."}),
        ];
        let (instructions, items) = convert_messages(&msgs);
        assert_eq!(instructions, "You are a helper.");
        assert_eq!(items.len(), 1, "system message should not appear in items");
    }

    #[test]
    fn test_multiple_system_messages_joined() {
        let msgs = vec![
            serde_json::json!({"role": "system", "content": "First part."}),
            serde_json::json!({"role": "system", "content": "Second part."}),
        ];
        let (instructions, _) = convert_messages(&msgs);
        assert_eq!(instructions, "First part.\n\nSecond part.");
    }

    #[test]
    fn test_assistant_with_tool_calls_emits_function_call_items() {
        let msgs = vec![serde_json::json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {
                    "id": "call_123",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": "{\"location\":\"Paris\"}"
                    }
                }
            ]
        })];
        let (_, items) = convert_messages(&msgs);
        // Empty content → no EasyMessage emitted; only the function_call item.
        assert_eq!(items.len(), 1);
        match &items[0] {
            InputItem::Item(Item::FunctionCall(fc)) => {
                assert_eq!(fc.call_id, "call_123");
                assert_eq!(fc.name, "get_weather");
                assert_eq!(fc.arguments, "{\"location\":\"Paris\"}");
            }
            other => panic!("expected FunctionCall item, got {other:?}"),
        }
    }

    #[test]
    fn test_assistant_with_text_and_tool_calls_emits_both() {
        let msgs = vec![serde_json::json!({
            "role": "assistant",
            "content": "Let me check that.",
            "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "lookup", "arguments": "{}"}}
            ]
        })];
        let (_, items) = convert_messages(&msgs);
        assert_eq!(items.len(), 2, "expected one message + one function_call");
        // First: EasyMessage with Commentary phase
        match &items[0] {
            InputItem::EasyMessage(e) => {
                assert_eq!(e.phase, Some(MessagePhase::Commentary));
            }
            other => panic!("expected EasyMessage first, got {other:?}"),
        }
        // Second: FunctionCall Item
        assert!(matches!(items[1], InputItem::Item(Item::FunctionCall(_))));
    }

    #[test]
    fn test_tool_message_becomes_function_call_output() {
        let msgs = vec![serde_json::json!({
            "role": "tool",
            "tool_call_id": "call_123",
            "content": "{\"temperature\": 72}"
        })];
        let (_, items) = convert_messages(&msgs);
        assert_eq!(items.len(), 1);
        match &items[0] {
            InputItem::Item(Item::FunctionCallOutput(fo)) => {
                assert_eq!(fo.call_id, "call_123");
                match &fo.output {
                    FunctionCallOutput::Text(t) => assert_eq!(t, "{\"temperature\": 72}"),
                    _ => panic!("expected text output"),
                }
            }
            other => panic!("expected FunctionCallOutput item, got {other:?}"),
        }
    }

    #[test]
    fn test_content_array_shape_extracted() {
        let msg = serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "Part one. "},
                {"type": "text", "text": "Part two."}
            ]
        });
        assert_eq!(
            extract_text_content(&msg),
            Some("Part one. Part two.".to_string())
        );
    }

    // ── Phase inference / preservation ───────────────────────────────────

    #[test]
    fn test_phase_defaults_to_final_answer_for_text_only_assistant() {
        let msgs = vec![serde_json::json!({
            "role": "assistant",
            "content": "Here's the answer."
        })];
        let (_, items) = convert_messages(&msgs);
        match &items[0] {
            InputItem::EasyMessage(e) => {
                assert_eq!(e.phase, Some(MessagePhase::FinalAnswer));
            }
            other => panic!("expected EasyMessage, got {other:?}"),
        }
    }

    #[test]
    fn test_phase_preserved_from_responses_phase_field() {
        // Explicit responses_phase in extras overrides default inference.
        let msgs = vec![serde_json::json!({
            "role": "assistant",
            "content": "I'm about to look this up.",
            "responses_phase": "commentary"
        })];
        let (_, items) = convert_messages(&msgs);
        match &items[0] {
            InputItem::EasyMessage(e) => {
                assert_eq!(e.phase, Some(MessagePhase::Commentary));
            }
            other => panic!("expected EasyMessage, got {other:?}"),
        }
    }

    #[test]
    fn test_phase_explicit_final_answer_overrides_tool_call_inference() {
        // Even with tool_calls present, explicit responses_phase wins.
        let msgs = vec![serde_json::json!({
            "role": "assistant",
            "content": "Done.",
            "responses_phase": "final_answer",
            "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "noop", "arguments": "{}"}}
            ]
        })];
        let (_, items) = convert_messages(&msgs);
        match &items[0] {
            InputItem::EasyMessage(e) => {
                assert_eq!(e.phase, Some(MessagePhase::FinalAnswer));
            }
            other => panic!("expected EasyMessage, got {other:?}"),
        }
    }

    // ── Tool schema conversion ───────────────────────────────────────────

    #[test]
    fn test_tool_conversion_from_chat_completions_shape() {
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get weather for a location",
                "parameters": {
                    "type": "object",
                    "properties": {"location": {"type": "string"}},
                    "required": ["location"]
                }
            }
        })];
        let converted = convert_tools(&tools).unwrap();
        assert_eq!(converted.len(), 1);
        match &converted[0] {
            Tool::Function(f) => {
                assert_eq!(f.name, "get_weather");
                assert_eq!(f.description.as_deref(), Some("Get weather for a location"));
                assert!(f.parameters.is_some());
                assert_eq!(f.strict, Some(false));
            }
            other => panic!("expected Function tool, got {other:?}"),
        }
    }

    #[test]
    fn test_tool_conversion_from_flat_shape() {
        // Direct top-level shape (no nested `function` wrapper) also accepted.
        let tools = vec![serde_json::json!({
            "type": "function",
            "name": "get_weather",
            "description": "Flat-shape weather lookup",
            "parameters": {"type": "object", "properties": {}}
        })];
        let converted = convert_tools(&tools).unwrap();
        assert_eq!(converted.len(), 1);
        match &converted[0] {
            Tool::Function(f) => {
                assert_eq!(f.name, "get_weather");
                assert_eq!(f.description.as_deref(), Some("Flat-shape weather lookup"));
            }
            other => panic!("expected Function tool, got {other:?}"),
        }
    }

    #[test]
    fn test_non_function_tool_skipped_with_warning() {
        let tools = vec![
            serde_json::json!({"type": "web_search"}),
            serde_json::json!({"type": "function", "name": "keep_me", "parameters": {}}),
        ];
        let converted = convert_tools(&tools).unwrap();
        assert_eq!(converted.len(), 1, "non-function tools should be dropped");
        match &converted[0] {
            Tool::Function(f) => assert_eq!(f.name, "keep_me"),
            _ => panic!("expected the surviving function tool"),
        }
    }

    #[test]
    fn test_tool_without_name_errors() {
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {"description": "no name"}
        })];
        let result = convert_tools(&tools);
        assert!(result.is_err());
    }

    // ── Settings mapping ─────────────────────────────────────────────────

    #[test]
    fn test_map_effort_roundtrip() {
        assert_eq!(map_effort(E::None), ReasoningEffort::None);
        assert_eq!(map_effort(E::Minimal), ReasoningEffort::Minimal);
        assert_eq!(map_effort(E::Low), ReasoningEffort::Low);
        assert_eq!(map_effort(E::Medium), ReasoningEffort::Medium);
        assert_eq!(map_effort(E::High), ReasoningEffort::High);
        assert_eq!(map_effort(E::Xhigh), ReasoningEffort::Xhigh);
    }

    #[test]
    fn test_map_verbosity_roundtrip() {
        assert_eq!(map_verbosity(V::Low), Verbosity::Low);
        assert_eq!(map_verbosity(V::Medium), Verbosity::Medium);
        assert_eq!(map_verbosity(V::High), Verbosity::High);
    }

    // ── Provider construction ───────────────────────────────────────────

    #[test]
    fn test_provider_new_stores_fields() {
        let settings = OpenAIResponsesSettings {
            model: "gpt-5.4-mini".to_string(),
            reasoning_effort: E::Medium,
            verbosity: V::High,
            max_output_tokens: Some(2048),
            temperature: None,
            top_p: None,
        };
        let provider =
            OpenAIResponsesProvider::new("https://api.openai.com/v1", Some("sk-test"), settings);
        assert_eq!(provider.endpoint, "https://api.openai.com/v1");
        assert_eq!(provider.api_key.as_deref(), Some("sk-test"));
        assert_eq!(provider.settings.model, "gpt-5.4-mini");
        assert_eq!(provider.settings.reasoning_effort, E::Medium);
    }

    #[test]
    fn test_provider_name() {
        let provider = OpenAIResponsesProvider::new(
            "https://api.openai.com/v1",
            Some("sk-test"),
            OpenAIResponsesSettings::default(),
        );
        assert_eq!(provider.name(), "OpenAI Responses");
    }

    #[test]
    fn test_build_client_fails_without_api_key() {
        let provider = OpenAIResponsesProvider::new(
            "https://api.openai.com/v1",
            None,
            OpenAIResponsesSettings::default(),
        );
        let result = provider.build_client();
        assert!(result.is_err());
    }
}
