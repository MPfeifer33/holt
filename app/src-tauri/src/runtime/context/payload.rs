use super::budget::ContextBudget;
use super::truncation::truncate_tool_result;
use crate::runtime::agent::{ContentBlock, ConversationMessage, MessageRole};

/// Result of building a context payload.
#[derive(Debug)]
pub struct ContextPayload {
    pub messages: Vec<serde_json::Value>,
    pub total_estimated_tokens: u32,
    pub needs_compaction: bool,
    pub evicted_count: usize,
}

/// Build the context payload for an API call from conversation history.
///
/// Layout (PRD 5.6):
/// - Pinned top: system prompt + tool definitions
/// - Sliding window: conversation history (oldest first, evicted if over budget)
/// - Pinned bottom: last N tool results + current user message
pub fn build_context_payload(
    system_prompt: &str,
    tools: &[serde_json::Value],
    messages: &[ConversationMessage],
    budget: &ContextBudget,
    max_tool_result_tokens: u32,
    truncation_strategy: &str,
    compaction_threshold_percent: u32,
    enable_pinned_bottom: bool,
) -> ContextPayload {
    let char_ratio = budget.char_ratio;

    // --- Pinned top: system prompt + tool definitions ---
    let system_tokens = ContextBudget::estimate_tokens(system_prompt, char_ratio);
    let tools_text = if tools.is_empty() {
        String::new()
    } else {
        serde_json::to_string(tools).unwrap_or_default()
    };
    let tools_tokens = ContextBudget::estimate_tokens(&tools_text, char_ratio);
    let pinned_top_tokens = system_tokens + tools_tokens;

    // --- Pinned bottom: last user message only ---
    // Only enabled for OpenAI-compatible (ChatCompletions) protocol.
    // Anthropic's API requires strict tool_use→tool_result chronological order,
    // so pinned_bottom (which reorders messages) must be disabled for it.
    //
    // Historical note: this used to ALSO pin the last 3 tool results, but
    // that broke OpenAI's GPT-5 and o-series models. Their API strictly
    // requires that an assistant message with `tool_calls` be immediately
    // followed by the matching `tool` messages — pinning tool results to
    // the bottom of the payload separates them from their parent assistant
    // message and OpenAI rejects the request with 400 "an assistant message
    // with 'tool_calls' must be followed by tool messages". The sliding
    // window already contains tool results in their natural adjacent
    // position, and the eviction pair-preservation logic below keeps
    // tool_call/tool_result pairs together on eviction, so pinning them
    // separately was both unnecessary and actively harmful. Only the
    // last user message is pinned now — it's a safety net against
    // eviction in long conversations and it doesn't participate in any
    // adjacency requirement.
    let (pinned_bottom_msgs, pinned_bottom_tokens) = if enable_pinned_bottom {
        let mut pb_msgs: Vec<&ConversationMessage> = Vec::new();
        let mut pb_tokens = 0u32;
        let pb_budget = budget.pinned_bottom_max;

        // Collect last user message
        if let Some(last_user) = messages.iter().rev().find(|m| m.role == MessageRole::User) {
            let tokens = estimate_message_tokens(last_user, char_ratio, None, truncation_strategy);
            if pb_tokens + tokens <= pb_budget {
                pb_msgs.push(last_user);
                pb_tokens += tokens;
            }
        }

        (pb_msgs, pb_tokens)
    } else {
        (Vec::new(), 0u32)
    };

    // Collect indices of pinned bottom messages so we skip them in sliding window
    let pinned_bottom_ptrs: Vec<*const ConversationMessage> =
        pinned_bottom_msgs.iter().map(|m| *m as *const _).collect();

    // --- Sliding window: remaining messages ---
    let sliding_budget = budget.sliding_window.min(
        budget
            .available_input_tokens()
            .saturating_sub(pinned_top_tokens + pinned_bottom_tokens),
    );

    let mut sliding_msgs: Vec<&ConversationMessage> = Vec::new();
    let mut sliding_tokens = 0u32;
    let mut evicted_count = 0usize;

    // Collect non-pinned messages
    let mut candidate_msgs: Vec<&ConversationMessage> = messages
        .iter()
        .filter(|m| !pinned_bottom_ptrs.contains(&(*m as *const _)))
        .collect();

    // Evict from the front (oldest) if over budget
    let mut total_candidate_tokens: u32 = candidate_msgs
        .iter()
        .map(|m| estimate_message_tokens(m, char_ratio, None, truncation_strategy))
        .sum();

    while total_candidate_tokens > sliding_budget && candidate_msgs.len() > 1 {
        // Don't break tool_call/tool_result pairs: if we evict an assistant message
        // with tool_calls, also evict the following tool results
        let first = candidate_msgs[0];
        let _first_tokens = estimate_message_tokens(first, char_ratio, None, truncation_strategy);

        // Check if it's an assistant message with tool calls — evict the pair.
        // Only evict Tool messages whose tool_call_id matches one of the assistant's
        // tool call IDs, preventing orphaned results from unrelated tool calls.
        let mut evict_count = 1;
        if let Some(ref tool_calls) = first.tool_calls {
            let call_ids: std::collections::HashSet<&str> =
                tool_calls.iter().map(|tc| tc.id.as_str()).collect();
            while evict_count < candidate_msgs.len()
                && candidate_msgs[evict_count].role == MessageRole::Tool
                && candidate_msgs[evict_count]
                    .tool_call_id
                    .as_deref()
                    .map(|id| call_ids.contains(id))
                    .unwrap_or(false)
            {
                evict_count += 1;
            }
        }

        let evicted_tokens: u32 = candidate_msgs[..evict_count]
            .iter()
            .map(|m| estimate_message_tokens(m, char_ratio, None, truncation_strategy))
            .sum();

        candidate_msgs.drain(..evict_count);
        total_candidate_tokens = total_candidate_tokens.saturating_sub(evicted_tokens);
        evicted_count += evict_count;
    }

    for msg in &candidate_msgs {
        let tokens = estimate_message_tokens(msg, char_ratio, None, truncation_strategy);
        sliding_msgs.push(msg);
        sliding_tokens += tokens;
    }

    // --- Assemble final payload ---
    let mut payload_msgs = Vec::new();

    // System prompt
    if !system_prompt.is_empty() {
        payload_msgs.push(serde_json::json!({
            "role": "system",
            "content": system_prompt,
        }));
    }

    // Sliding window messages
    for msg in &sliding_msgs {
        payload_msgs.push(message_to_json(msg, None, truncation_strategy, char_ratio));
    }

    // Pinned bottom messages
    for msg in &pinned_bottom_msgs {
        // Only add if not already in sliding window
        let ptr = *msg as *const ConversationMessage;
        let already_added = sliding_msgs.iter().any(|m| std::ptr::eq(*m, ptr));
        if !already_added {
            payload_msgs.push(message_to_json(
                msg,
                Some(max_tool_result_tokens),
                truncation_strategy,
                char_ratio,
            ));
        }
    }

    let total_estimated_tokens = pinned_top_tokens + sliding_tokens + pinned_bottom_tokens;
    let needs_compaction =
        budget.needs_compaction(total_estimated_tokens, compaction_threshold_percent);

    ContextPayload {
        messages: payload_msgs,
        total_estimated_tokens,
        needs_compaction,
        evicted_count,
    }
}

pub fn message_to_json(
    msg: &ConversationMessage,
    max_tool_result_tokens: Option<u32>,
    truncation_strategy: &str,
    char_ratio: f32,
) -> serde_json::Value {
    let role_str = match msg.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };

    // Build content: use content_blocks if present, otherwise fall back to string
    let content_value = if let Some(blocks) = &msg.content_blocks {
        // Convert ContentBlock vec to provider-neutral JSON array
        let json_blocks: Vec<serde_json::Value> = blocks
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => {
                    serde_json::json!({"type": "text", "text": text})
                }
                ContentBlock::Image { data, media_type } => {
                    serde_json::json!({
                        "type": "image",
                        "data": data,
                        "media_type": media_type,
                    })
                }
            })
            .collect();
        serde_json::Value::Array(json_blocks)
    } else {
        // Existing behavior: plain string content with tool result truncation
        let content = if msg.role == MessageRole::Tool {
            if let Some(max) = max_tool_result_tokens {
                truncate_tool_result(&msg.content, max, char_ratio, truncation_strategy)
            } else {
                msg.content.clone()
            }
        } else {
            msg.content.clone()
        };
        serde_json::Value::String(content)
    };

    // OpenAI spec: assistant messages with tool_calls and empty content should
    // send content: null, not content: "". Some providers reject empty strings.
    let final_content = if msg.role == MessageRole::Assistant
        && msg.tool_calls.is_some()
        && msg.content.is_empty()
        && msg.content_blocks.is_none()
    {
        serde_json::Value::Null
    } else {
        content_value
    };

    let mut m = serde_json::json!({
        "role": role_str,
        "content": final_content,
    });

    if let Some(tool_calls) = &msg.tool_calls {
        m["tool_calls"] = serde_json::json!(tool_calls
            .iter()
            .map(|tc| {
                serde_json::json!({
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.name,
                        "arguments": tc.arguments.to_string(),
                    }
                })
            })
            .collect::<Vec<_>>());
    }

    if let Some(tool_call_id) = &msg.tool_call_id {
        m["tool_call_id"] = serde_json::json!(tool_call_id);
    }

    // Pass thinking_content through for Anthropic provider
    if let Some(thinking) = &msg.thinking_content {
        m["thinking_content"] = serde_json::json!(thinking);
    }

    // Pass Responses API metadata through for OpenAIResponsesProvider.
    // Chat Completions providers ignore these extra fields (they're not in
    // the Chat Completions schema and get silently dropped). The Responses
    // provider reads them during its own conversion to Items so the
    // assistant `phase` tag and preserved chain-of-thought reasoning items
    // survive across turns.
    if let Some(phase) = &msg.responses_phase {
        m["responses_phase"] = serde_json::json!(phase);
    }
    if let Some(reasoning) = &msg.reasoning_items {
        m["reasoning_items"] = reasoning.clone();
    }

    m
}

fn estimate_message_tokens(
    msg: &ConversationMessage,
    char_ratio: f32,
    max_tool_result_tokens: Option<u32>,
    _truncation_strategy: &str,
) -> u32 {
    let content_tokens = if msg.role == MessageRole::Tool {
        if let Some(max) = max_tool_result_tokens {
            ContextBudget::estimate_tokens(&msg.content, char_ratio).min(max)
        } else {
            ContextBudget::estimate_tokens(&msg.content, char_ratio)
        }
    } else {
        ContextBudget::estimate_tokens(&msg.content, char_ratio)
    };

    // Add overhead for tool calls metadata
    let tool_call_tokens = msg
        .tool_calls
        .as_ref()
        .map(|tcs| {
            tcs.iter()
                .map(|tc| {
                    ContextBudget::estimate_tokens(&tc.name, char_ratio)
                        + ContextBudget::estimate_tokens(&tc.arguments.to_string(), char_ratio)
                        + 10 // overhead for id, type, etc.
                })
                .sum::<u32>()
        })
        .unwrap_or(0);

    // Add image token estimates (~750 tokens per image, rough approximation based on Anthropic pricing)
    let image_tokens: u32 = msg
        .content_blocks
        .as_ref()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Image { .. } => Some(750u32),
                    _ => None,
                })
                .sum()
        })
        .unwrap_or(0);

    content_tokens + tool_call_tokens + image_tokens + 4 // 4 tokens overhead per message (role, separators)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::agent::ToolCallRecord;
    use chrono::Utc;

    #[test]
    fn test_build_context_simple() {
        let budget = ContextBudget::default();
        let messages = vec![
            ConversationMessage {
                role: MessageRole::User,
                content: "Hello".to_string(),
                content_blocks: None,
                timestamp: Utc::now(),
                token_estimate: None,
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            },
            ConversationMessage {
                role: MessageRole::Assistant,
                content: "Hi there!".to_string(),
                content_blocks: None,
                timestamp: Utc::now(),
                token_estimate: None,
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            },
        ];

        let payload = build_context_payload(
            "You are helpful.",
            &[],
            &messages,
            &budget,
            2000,
            "head_tail",
            80,
            false,
        );

        assert!(payload.messages.len() >= 2); // system + at least some messages
        assert_eq!(payload.evicted_count, 0);
        assert!(!payload.needs_compaction);
    }

    #[test]
    fn test_eviction_keeps_pairs() {
        // Create a small budget that forces eviction
        let budget = ContextBudget {
            total: 200,
            reserved_for_response: 20,
            pinned_top_max: 20,
            pinned_bottom_max: 20,
            sliding_window: 60,
            char_ratio: 1.0, // 1 char = 1 token for easy testing
        };

        let mut messages = Vec::new();
        // Old assistant message with tool call + tool result (should be evicted as pair)
        messages.push(ConversationMessage {
            role: MessageRole::Assistant,
            content: "I'll run a command.".to_string(),
            content_blocks: None,
            timestamp: Utc::now(),
            token_estimate: None,
            tool_calls: Some(vec![ToolCallRecord {
                id: "call_1".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({"cmd": "ls"}),
            }]),
            tool_call_id: None,
            outcome_tag: None,
            thinking_content: None,
            responses_phase: None,
            reasoning_items: None,
        });
        messages.push(ConversationMessage {
            role: MessageRole::Tool,
            content: "file1.txt file2.txt".to_string(),
            content_blocks: None,
            timestamp: Utc::now(),
            token_estimate: None,
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
            outcome_tag: None,
            thinking_content: None,
            responses_phase: None,
            reasoning_items: None,
        });
        // Recent user message
        messages.push(ConversationMessage {
            role: MessageRole::User,
            content: "Thanks!".to_string(),
            content_blocks: None,
            timestamp: Utc::now(),
            token_estimate: None,
            tool_calls: None,
            tool_call_id: None,
            outcome_tag: None,
            thinking_content: None,
            responses_phase: None,
            reasoning_items: None,
        });

        let payload = build_context_payload(
            "System",
            &[],
            &messages,
            &budget,
            50,
            "head_tail",
            80,
            false,
        );

        // The assistant+tool pair should be evicted together (count=2), not split
        assert!(payload.evicted_count == 0 || payload.evicted_count == 2);
    }

    #[test]
    fn test_message_to_json_with_content_blocks() {
        let msg = ConversationMessage {
            role: MessageRole::User,
            content: "What's in this image?".to_string(),
            content_blocks: Some(vec![
                ContentBlock::Text {
                    text: "What's in this image?".to_string(),
                },
                ContentBlock::Image {
                    data: "iVBOR".to_string(),
                    media_type: "image/png".to_string(),
                },
            ]),
            timestamp: Utc::now(),
            token_estimate: None,
            tool_calls: None,
            tool_call_id: None,
            outcome_tag: None,
            thinking_content: None,
            responses_phase: None,
            reasoning_items: None,
        };
        let json = message_to_json(&msg, None, "end", 3.5);
        let content = json.get("content").unwrap();
        assert!(content.is_array());
        let arr = content.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[1]["type"], "image");
    }

    #[test]
    fn test_message_to_json_without_content_blocks_unchanged() {
        let msg = ConversationMessage {
            role: MessageRole::User,
            content: "Hello".to_string(),
            content_blocks: None,
            timestamp: Utc::now(),
            token_estimate: None,
            tool_calls: None,
            tool_call_id: None,
            outcome_tag: None,
            thinking_content: None,
            responses_phase: None,
            reasoning_items: None,
        };
        let json = message_to_json(&msg, None, "end", 3.5);
        let content = json.get("content").unwrap();
        assert!(content.is_string());
        assert_eq!(content.as_str().unwrap(), "Hello");
    }
}
