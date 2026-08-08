//! Codex V2 event parser and mapper.
//!
//! Parses raw NDJSON notifications from the transport layer (`codex_server::CodexEvent`)
//! into typed event variants, and maps them to Holt `StreamEvent`s for the frontend.

use serde_json::Value;

use crate::providers::types::{StreamEvent, StreamEventType};

use super::codex_types::*;

// =============================================================================
// Typed event enum
// =============================================================================

/// A parsed Codex V2 notification event.
///
/// Each variant wraps the corresponding typed notification struct from `codex_types`.
/// The `Unknown` variant ensures forward compatibility — new event types from future
/// Codex versions are captured without crashing.
#[derive(Debug, Clone)]
pub enum TypedCodexEvent {
    ThreadStarted(ThreadStartedNotification),
    TurnStarted(TurnStartedNotification),
    AgentMessageDelta(AgentMessageDeltaNotification),
    TurnPlanUpdated(TurnPlanUpdatedNotification),
    TurnCompleted(TurnCompletedNotification),
    TokenUsageUpdated(ThreadTokenUsageUpdatedNotification),
    GoalUpdated(ThreadGoalUpdatedNotification),
    RateLimitUpdated(AccountRateLimitsUpdatedNotification),
    ApprovalRequested(ApprovalReviewStartedNotification),
    CommandOutput(CommandOutputDeltaNotification),
    McpToolProgress(McpToolCallProgressNotification),
    McpServerStatus(McpServerStatusUpdatedNotification),
    ContextCompacted(ContextCompactedNotification),
    DiffUpdated(TurnDiffUpdatedNotification),
    ThreadStatusChanged(ThreadStatusChangedNotification),
    ThreadNameUpdated(ThreadNameUpdatedNotification),
    ItemStarted(ItemNotification),
    ItemCompleted(ItemNotification),
    Error(ErrorNotification),
    /// Forward-compat: an event type we don't recognize yet.
    Unknown {
        method: String,
        params: Value,
    },
}

// =============================================================================
// Parsing
// =============================================================================

/// Parse a notification method + params into a typed event.
///
/// This is the central dispatch point. The method name comes from the JSON-RPC
/// notification's `method` field, and params is the `params` object.
pub fn parse_notification(method: &str, params: Value) -> TypedCodexEvent {
    match method {
        "thread/started" => try_parse(params, TypedCodexEvent::ThreadStarted, method),
        "turn/started" => try_parse(params, TypedCodexEvent::TurnStarted, method),
        "item/agentMessage/delta" => try_parse(params, TypedCodexEvent::AgentMessageDelta, method),
        "turn/plan/updated" => try_parse(params, TypedCodexEvent::TurnPlanUpdated, method),
        "turn/completed" => try_parse(params, TypedCodexEvent::TurnCompleted, method),
        "thread/tokenUsage/updated" => {
            try_parse(params, TypedCodexEvent::TokenUsageUpdated, method)
        }
        "thread/goal/updated" => try_parse(params, TypedCodexEvent::GoalUpdated, method),
        "account/rateLimits/updated" => {
            try_parse(params, TypedCodexEvent::RateLimitUpdated, method)
        }
        "item/autoApprovalReview/started" => {
            try_parse(params, TypedCodexEvent::ApprovalRequested, method)
        }
        "item/commandExecution/outputDelta" => {
            try_parse(params, TypedCodexEvent::CommandOutput, method)
        }
        "item/mcpToolCall/progress" => try_parse(params, TypedCodexEvent::McpToolProgress, method),
        "mcpServer/startupStatus/updated" => {
            try_parse(params, TypedCodexEvent::McpServerStatus, method)
        }
        "thread/compacted" => try_parse(params, TypedCodexEvent::ContextCompacted, method),
        "turn/diff/updated" => try_parse(params, TypedCodexEvent::DiffUpdated, method),
        "thread/status/changed" => try_parse(params, TypedCodexEvent::ThreadStatusChanged, method),
        "thread/name/updated" => try_parse(params, TypedCodexEvent::ThreadNameUpdated, method),
        "item/started" => try_parse(params, TypedCodexEvent::ItemStarted, method),
        "item/completed" => try_parse(params, TypedCodexEvent::ItemCompleted, method),
        "error" => try_parse(params, TypedCodexEvent::Error, method),
        _ => TypedCodexEvent::Unknown {
            method: method.to_string(),
            params,
        },
    }
}

/// Try to deserialize params into the typed notification struct.
/// On failure, fall back to Unknown with a warning log.
fn try_parse<T, F>(params: Value, constructor: F, method: &str) -> TypedCodexEvent
where
    T: serde::de::DeserializeOwned,
    F: FnOnce(T) -> TypedCodexEvent,
{
    match serde_json::from_value::<T>(params.clone()) {
        Ok(typed) => constructor(typed),
        Err(e) => {
            tracing::warn!(
                method,
                error = %e,
                "Failed to parse known notification type — treating as Unknown"
            );
            TypedCodexEvent::Unknown {
                method: method.to_string(),
                params,
            }
        }
    }
}

// =============================================================================
// StreamEvent mapping
// =============================================================================

/// Map a typed Codex event to a Holt `StreamEvent` for frontend display.
///
/// Returns `None` for events that are internal-only and don't need UI representation
/// (e.g., rate limits are handled by the transport layer's state tracking).
pub fn to_stream_event(event: &TypedCodexEvent, agent_id: &str) -> Option<StreamEvent> {
    match event {
        // The frontend consumers (ChatTile, activityFeed) read `data.text`,
        // matching every other lane (anthropic, agent_sdk, openai). `content`
        // renders as invisible text — keep the key as `text`.
        TypedCodexEvent::AgentMessageDelta(delta) => Some(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Token,
            data: serde_json::json!({
                "text": delta.delta,
                "threadId": delta.thread_id,
                "turnId": delta.turn_id,
            }),
        }),

        TypedCodexEvent::TurnCompleted(completed) => Some(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Done,
            data: serde_json::json!({
                "threadId": completed.thread_id,
                "turn": completed.turn,
            }),
        }),

        TypedCodexEvent::Error(err) => Some(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Error,
            data: serde_json::json!({
                "message": err.error.message,
                "threadId": err.thread_id,
                "turnId": err.turn_id,
                "willRetry": err.will_retry,
            }),
        }),

        TypedCodexEvent::ApprovalRequested(approval) => Some(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::ToolCall,
            data: serde_json::json!({
                "type": "approval_required",
                "reviewId": approval.review_id,
                "action": approval.action,
                "threadId": approval.thread_id,
            }),
        }),

        // CommandOutput uses `extra` (serde flatten) for the delta content
        TypedCodexEvent::CommandOutput(output) => Some(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::ToolResult,
            data: serde_json::json!({
                "type": "command_output",
                "threadId": output.thread_id,
                "extra": output.extra,
            }),
        }),

        // TurnPlanUpdated uses `extra` (serde flatten) for plan content
        TypedCodexEvent::TurnPlanUpdated(plan) => Some(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Thinking,
            data: serde_json::json!({
                "threadId": plan.thread_id,
                "turnId": plan.turn_id,
                "extra": plan.extra,
            }),
        }),

        TypedCodexEvent::ContextCompacted(compacted) => Some(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::ContextWarning,
            data: serde_json::json!({
                "type": "context_compacted",
                "threadId": compacted.thread_id,
                "turnId": compacted.turn_id,
            }),
        }),

        // ThreadStatusChanged uses `extra` for status details
        TypedCodexEvent::ThreadStatusChanged(status) => Some(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::ConversationUpdated,
            data: serde_json::json!({
                "type": "thread_status_changed",
                "threadId": status.thread_id,
                "extra": status.extra,
            }),
        }),

        // McpToolProgress uses `extra` for progress details
        TypedCodexEvent::McpToolProgress(progress) => Some(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::ToolCall,
            data: serde_json::json!({
                "type": "mcp_tool_progress",
                "threadId": progress.thread_id,
                "extra": progress.extra,
            }),
        }),

        // DiffUpdated uses `extra` for diff content
        TypedCodexEvent::DiffUpdated(diff) => Some(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::ConversationUpdated,
            data: serde_json::json!({
                "type": "diff_updated",
                "threadId": diff.thread_id,
                "extra": diff.extra,
            }),
        }),

        // ThreadNameUpdated uses `extra` for the name
        TypedCodexEvent::ThreadNameUpdated(name) => Some(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::ConversationUpdated,
            data: serde_json::json!({
                "type": "thread_renamed",
                "threadId": name.thread_id,
                "extra": name.extra,
            }),
        }),

        // Tool-call items carry the real tool names — map to the same data
        // shape the frontend reads for other lanes: {tool, tool_call_id,
        // arguments, status, result}. "unknown" in the activity feed means
        // one of these fields went missing.
        TypedCodexEvent::ItemStarted(n) => item_tool_event(n, agent_id, false),
        TypedCodexEvent::ItemCompleted(n) => item_tool_event(n, agent_id, true),

        // Internal events — tracked by transport layer or Phase 5, not for frontend
        TypedCodexEvent::ThreadStarted(_)
        | TypedCodexEvent::TurnStarted(_)
        | TypedCodexEvent::TokenUsageUpdated(_)
        | TypedCodexEvent::GoalUpdated(_)
        | TypedCodexEvent::RateLimitUpdated(_)
        | TypedCodexEvent::McpServerStatus(_)
        | TypedCodexEvent::Unknown { .. } => None,
    }
}

/// Map an item/started or item/completed notification to a ToolCall /
/// ToolResult StreamEvent for tool-shaped items. Non-tool items (reasoning,
/// plan, agentMessage — whose text flows via deltas and final_response)
/// return None.
fn item_tool_event(n: &ItemNotification, agent_id: &str, completed: bool) -> Option<StreamEvent> {
    let event_type = if completed {
        StreamEventType::ToolResult
    } else {
        StreamEventType::ToolCall
    };

    let data = match &n.item {
        ThreadItem::McpToolCall {
            id,
            tool,
            server,
            status,
            arguments,
            result,
            error,
        } => serde_json::json!({
            "type": "mcp_tool_call",
            "tool": tool,
            "tool_call_id": id,
            "server": server,
            "status": status,
            "arguments": arguments,
            "result": result,
            "error": error,
            "threadId": n.thread_id,
        }),
        ThreadItem::DynamicToolCall {
            id,
            tool,
            status,
            arguments,
        } => serde_json::json!({
            "type": "dynamic_tool_call",
            "tool": tool,
            "tool_call_id": id,
            "status": status,
            "arguments": arguments,
            "threadId": n.thread_id,
        }),
        ThreadItem::CommandExecution {
            id,
            command,
            status,
            exit_code,
            aggregated_output,
        } => serde_json::json!({
            "type": "command_execution",
            "tool": "shell",
            "tool_call_id": id,
            "command": command,
            "status": status,
            "exit_code": exit_code,
            "result": aggregated_output,
            "threadId": n.thread_id,
        }),
        ThreadItem::WebSearch { id, query } => serde_json::json!({
            "type": "web_search",
            "tool": "web_search",
            "tool_call_id": id,
            "arguments": { "query": query },
            "threadId": n.thread_id,
        }),
        ThreadItem::AgentMessage { .. } | ThreadItem::Other => return None,
    };

    Some(StreamEvent {
        agent_id: agent_id.to_string(),
        event_type,
        data,
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Real payloads captured from `codex app-server --stdio` (codex-cli
    /// 0.142.5): initialize, ephemeral thread/start, one turn to completion.
    /// Recapture with scratchpad capture script if the CLI is upgraded.
    const CAPTURE: &str = include_str!("testdata/codex_appserver_capture.json");

    /// Typed structs must parse the REAL wire payloads — drift here has
    /// hard-failed turns before (ThreadStatus modeled wrong → thread/start
    /// response unparseable → every send failed).
    #[test]
    fn real_capture_responses_parse_into_typed_structs() {
        let capture: Value = serde_json::from_str(CAPTURE).unwrap();

        let thread_resp: ThreadResponse =
            serde_json::from_value(capture["thread_start_result"].clone())
                .expect("thread/start result must parse as ThreadResponse");
        assert!(matches!(thread_resp.thread.status, ThreadStatus::Idle));

        let _turn_resp: TurnStartResponse =
            serde_json::from_value(capture["turn_start_result"].clone())
                .expect("turn/start result must parse as TurnStartResponse");
    }

    /// Every captured notification we have a typed parser for must parse —
    /// try_parse degrades to Unknown on failure, which silently breaks
    /// streaming (e.g. an unparseable turn/completed hangs the turn).
    #[test]
    fn real_capture_notifications_parse_as_typed_events() {
        let capture: Value = serde_json::from_str(CAPTURE).unwrap();
        // Methods present in parse_notification's dispatch table.
        let dispatched = [
            "thread/started",
            "turn/started",
            "item/agentMessage/delta",
            "item/started",
            "item/completed",
            "turn/plan/updated",
            "turn/completed",
            "thread/tokenUsage/updated",
            "thread/goal/updated",
            "account/rateLimits/updated",
            "item/autoApprovalReview/started",
            "item/commandExecution/outputDelta",
            "item/mcpToolCall/progress",
            "mcpServer/startupStatus/updated",
            "thread/compacted",
            "turn/diff/updated",
            "thread/status/changed",
            "thread/name/updated",
        ];

        let mut checked = 0;
        for n in capture["notifications"].as_array().unwrap() {
            let method = n["method"].as_str().unwrap();
            if !dispatched.contains(&method) {
                continue;
            }
            let event = parse_notification(method, n["params"].clone());
            assert!(
                !matches!(event, TypedCodexEvent::Unknown { .. }),
                "typed parse failed for real {method} payload: {}",
                n["params"]
            );
            checked += 1;
        }
        // The capture contains at least these dispatched methods; if this
        // drops to zero the fixture is broken, not the parsers.
        assert!(
            checked >= 5,
            "only {checked} dispatched notifications in fixture"
        );
    }

    // --- Parsing tests ---

    #[test]
    fn parse_agent_message_delta() {
        let params = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "itemId": "item_1",
            "delta": "Hello world"
        });
        let event = parse_notification("item/agentMessage/delta", params);
        match event {
            TypedCodexEvent::AgentMessageDelta(d) => {
                assert_eq!(d.delta, "Hello world");
                assert_eq!(d.thread_id, "t_1");
                assert_eq!(d.turn_id, "turn_1");
                assert_eq!(d.item_id, "item_1");
            }
            other => panic!("Expected AgentMessageDelta, got {:?}", other),
        }
    }

    #[test]
    fn parse_turn_completed() {
        let params = json!({
            "threadId": "t_1",
            "turn": {
                "id": "turn_1",
                "items": []
            }
        });
        let event = parse_notification("turn/completed", params);
        match event {
            TypedCodexEvent::TurnCompleted(tc) => {
                assert_eq!(tc.thread_id, "t_1");
                assert_eq!(tc.turn["id"], "turn_1");
            }
            other => panic!("Expected TurnCompleted, got {:?}", other),
        }
    }

    #[test]
    fn parse_thread_started() {
        let params = json!({
            "thread": {
                "id": "t_1",
                "sessionId": "s_1",
                "createdAt": 0,
                "updatedAt": 0,
                "cwd": "/tmp",
                "modelProvider": "openai",
                "preview": "",
                "status": {"type": "idle"},
                "ephemeral": false,
                "cliVersion": "0.142.5"
            }
        });
        let event = parse_notification("thread/started", params);
        match event {
            TypedCodexEvent::ThreadStarted(ts) => {
                assert_eq!(ts.thread.id, "t_1");
            }
            other => panic!("Expected ThreadStarted, got {:?}", other),
        }
    }

    #[test]
    fn parse_turn_started() {
        let params = json!({
            "threadId": "t_1",
            "turn": { "id": "turn_1" }
        });
        let event = parse_notification("turn/started", params);
        match event {
            TypedCodexEvent::TurnStarted(ts) => {
                assert_eq!(ts.thread_id, "t_1");
            }
            other => panic!("Expected TurnStarted, got {:?}", other),
        }
    }

    #[test]
    fn parse_error_notification() {
        let params = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "error": {
                "message": "Something went wrong"
            },
            "willRetry": false
        });
        let event = parse_notification("error", params);
        match event {
            TypedCodexEvent::Error(e) => {
                assert_eq!(e.error.message, "Something went wrong");
                assert!(!e.will_retry);
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn parse_token_usage_updated() {
        let params = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "tokenUsage": {
                "last": {
                    "inputTokens": 100,
                    "outputTokens": 50,
                    "cachedInputTokens": 0,
                    "reasoningOutputTokens": 0,
                    "totalTokens": 150
                },
                "total": {
                    "inputTokens": 500,
                    "outputTokens": 200,
                    "cachedInputTokens": 0,
                    "reasoningOutputTokens": 0,
                    "totalTokens": 700
                },
                "modelContextWindow": 128000
            }
        });
        let event = parse_notification("thread/tokenUsage/updated", params);
        match event {
            TypedCodexEvent::TokenUsageUpdated(u) => {
                assert_eq!(u.token_usage.last.total_tokens, 150);
                assert_eq!(u.token_usage.model_context_window, Some(128000));
            }
            other => panic!("Expected TokenUsageUpdated, got {:?}", other),
        }
    }

    #[test]
    fn parse_goal_updated() {
        let params = json!({
            "threadId": "t_1",
            "goal": {
                "threadId": "t_1",
                "objective": "Build the feature",
                "status": "active",
                "tokenBudget": 50000,
                "tokensUsed": 12000,
                "timeUsedSeconds": 60,
                "createdAt": 1720000000000_i64,
                "updatedAt": 1720000001000_i64
            }
        });
        let event = parse_notification("thread/goal/updated", params);
        match event {
            TypedCodexEvent::GoalUpdated(g) => {
                assert_eq!(g.goal.objective, "Build the feature");
                assert!(matches!(g.goal.status, ThreadGoalStatus::Active));
            }
            other => panic!("Expected GoalUpdated, got {:?}", other),
        }
    }

    #[test]
    fn parse_approval_requested() {
        let params = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "reviewId": "review_1",
            "action": {"type": "command", "command": "ls", "cwd": "/home", "source": "shell"},
            "review": {"status": "inProgress"},
            "startedAtMs": 1_720_000_000_000_i64
        });
        let event = parse_notification("item/autoApprovalReview/started", params);
        assert!(matches!(event, TypedCodexEvent::ApprovalRequested(_)));
    }

    #[test]
    fn parse_command_output_delta() {
        let params = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "delta": "total 42\n"
        });
        let event = parse_notification("item/commandExecution/outputDelta", params);
        match event {
            TypedCodexEvent::CommandOutput(o) => {
                assert_eq!(o.thread_id, "t_1");
                // delta is in the extra field via serde flatten
                assert_eq!(o.extra["delta"], "total 42\n");
            }
            other => panic!("Expected CommandOutput, got {:?}", other),
        }
    }

    #[test]
    fn parse_context_compacted() {
        let params = json!({
            "threadId": "t_1",
            "turnId": "turn_1"
        });
        let event = parse_notification("thread/compacted", params);
        match event {
            TypedCodexEvent::ContextCompacted(c) => {
                assert_eq!(c.thread_id, "t_1");
                assert_eq!(c.turn_id, "turn_1");
            }
            other => panic!("Expected ContextCompacted, got {:?}", other),
        }
    }

    #[test]
    fn parse_mcp_server_status() {
        let params = json!({
            "name": "holt",
            "status": "ready"
        });
        let event = parse_notification("mcpServer/startupStatus/updated", params);
        match event {
            TypedCodexEvent::McpServerStatus(s) => {
                assert_eq!(s.name, "holt");
                assert!(matches!(s.status, McpServerStartupState::Ready));
            }
            other => panic!("Expected McpServerStatus, got {:?}", other),
        }
    }

    #[test]
    fn parse_thread_name_updated() {
        let params = json!({
            "threadId": "t_1",
            "name": "My Cool Thread"
        });
        let event = parse_notification("thread/name/updated", params);
        match event {
            TypedCodexEvent::ThreadNameUpdated(n) => {
                assert_eq!(n.thread_id, "t_1");
                // name is in extra via serde flatten
                assert_eq!(n.extra["name"], "My Cool Thread");
            }
            other => panic!("Expected ThreadNameUpdated, got {:?}", other),
        }
    }

    #[test]
    fn parse_rate_limit_updated() {
        // AccountRateLimitsUpdatedNotification wraps a RateLimitSnapshot
        // inside a `rateLimits` field
        let params = json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 50,
                    "resetsAt": 1_720_000_000_000_i64,
                    "windowDurationMins": 60
                }
            }
        });
        let event = parse_notification("account/rateLimits/updated", params);
        match &event {
            TypedCodexEvent::RateLimitUpdated(r) => {
                let primary = r.rate_limits.primary.as_ref().unwrap();
                assert_eq!(primary.used_percent, 50);
            }
            other => panic!("Expected RateLimitUpdated, got {:?}", other),
        }
    }

    #[test]
    fn unknown_method_returns_unknown_variant() {
        let params = json!({"foo": "bar"});
        let event = parse_notification("SomeNewFutureNotification", params.clone());
        match event {
            TypedCodexEvent::Unknown { method, params: p } => {
                assert_eq!(method, "SomeNewFutureNotification");
                assert_eq!(p, params);
            }
            other => panic!("Expected Unknown, got {:?}", other),
        }
    }

    #[test]
    fn malformed_params_falls_back_to_unknown() {
        // AgentMessageDelta requires threadId, turnId, itemId, delta — send empty object
        let params = json!({});
        let event = parse_notification("item/agentMessage/delta", params);
        // Should fall back to Unknown due to missing required fields
        assert!(matches!(event, TypedCodexEvent::Unknown { .. }));
    }

    // --- StreamEvent mapping tests ---

    #[test]
    fn delta_maps_to_token_event() {
        let event = TypedCodexEvent::AgentMessageDelta(AgentMessageDeltaNotification {
            thread_id: "t_1".to_string(),
            turn_id: "turn_1".to_string(),
            item_id: "item_1".to_string(),
            delta: "Hi".to_string(),
        });
        let stream = to_stream_event(&event, "agent-1").unwrap();
        assert_eq!(stream.event_type, StreamEventType::Token);
        assert_eq!(stream.agent_id, "agent-1");
        // Frontend consumers (ChatTile, activityFeed) read data.text — the
        // same key every other lane emits. "content" rendered as nothing.
        assert_eq!(stream.data["text"], "Hi");
    }

    #[test]
    fn item_completed_agent_message_parses_and_stays_internal() {
        // Shape from the real capture: the agentMessage item/completed
        // carries the authoritative full reply text.
        let params = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "item": { "type": "agentMessage", "id": "i_2", "text": "OK", "phase": null }
        });
        let event = parse_notification("item/completed", params);
        match &event {
            TypedCodexEvent::ItemCompleted(n) => match &n.item {
                ThreadItem::AgentMessage { text, .. } => assert_eq!(text, "OK"),
                other => panic!("expected AgentMessage, got {other:?}"),
            },
            other => panic!("expected ItemCompleted, got {other:?}"),
        }
        // Text flows via Token deltas + final_response — no duplicate UI event
        assert!(to_stream_event(&event, "agent-1").is_none());
    }

    #[test]
    fn mcp_tool_call_items_map_to_tool_events_with_name() {
        // Field set per the generated protocol schema for mcpToolCall items
        let started = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "item": {
                "type": "mcpToolCall",
                "id": "i_3",
                "tool": "recall",
                "server": "holt",
                "status": "inProgress",
                "arguments": { "query": "hillock" }
            }
        });
        let event = parse_notification("item/started", started);
        let stream = to_stream_event(&event, "agent-1").unwrap();
        assert_eq!(stream.event_type, StreamEventType::ToolCall);
        assert_eq!(stream.data["tool"], "recall");
        assert_eq!(stream.data["tool_call_id"], "i_3");

        let completed = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "item": {
                "type": "mcpToolCall",
                "id": "i_3",
                "tool": "recall",
                "server": "holt",
                "status": "completed",
                "result": { "memories": [] }
            }
        });
        let event = parse_notification("item/completed", completed);
        let stream = to_stream_event(&event, "agent-1").unwrap();
        assert_eq!(stream.event_type, StreamEventType::ToolResult);
        assert_eq!(stream.data["tool"], "recall");
        assert_eq!(stream.data["status"], "completed");
    }

    #[test]
    fn unmodeled_item_types_parse_as_other() {
        // reasoning items appear in every turn (see capture) — must not
        // degrade the whole notification to Unknown
        let params = json!({
            "threadId": "t_1",
            "item": { "type": "reasoning", "id": "i_1", "summary": [], "content": [] }
        });
        let event = parse_notification("item/started", params);
        match &event {
            TypedCodexEvent::ItemStarted(n) => assert!(matches!(n.item, ThreadItem::Other)),
            other => panic!("expected ItemStarted, got {other:?}"),
        }
        assert!(to_stream_event(&event, "agent-1").is_none());
    }

    #[test]
    fn turn_completed_maps_to_done() {
        let event = TypedCodexEvent::TurnCompleted(TurnCompletedNotification {
            thread_id: "t_1".to_string(),
            turn: json!({"id": "turn_1"}),
        });
        let stream = to_stream_event(&event, "agent-1").unwrap();
        assert_eq!(stream.event_type, StreamEventType::Done);
    }

    #[test]
    fn error_maps_to_error_event() {
        let event = TypedCodexEvent::Error(ErrorNotification {
            thread_id: "t_1".to_string(),
            turn_id: "turn_1".to_string(),
            error: TurnError {
                message: "boom".to_string(),
                additional_details: None,
                codex_error_info: None,
            },
            will_retry: false,
        });
        let stream = to_stream_event(&event, "agent-1").unwrap();
        assert_eq!(stream.event_type, StreamEventType::Error);
        assert_eq!(stream.data["message"], "boom");
    }

    #[test]
    fn plan_updated_maps_to_thinking() {
        let event = TypedCodexEvent::TurnPlanUpdated(TurnPlanUpdatedNotification {
            thread_id: "t_1".to_string(),
            turn_id: Some("turn_1".to_string()),
            extra: json!({"plan": ["step 1", "step 2"]}),
        });
        let stream = to_stream_event(&event, "agent-1").unwrap();
        assert_eq!(stream.event_type, StreamEventType::Thinking);
    }

    #[test]
    fn context_compacted_maps_to_context_warning() {
        let event = TypedCodexEvent::ContextCompacted(ContextCompactedNotification {
            thread_id: "t_1".to_string(),
            turn_id: "turn_1".to_string(),
        });
        let stream = to_stream_event(&event, "agent-1").unwrap();
        assert_eq!(stream.event_type, StreamEventType::ContextWarning);
    }

    #[test]
    fn internal_events_return_none() {
        // Use parse_notification to construct ThreadStarted — avoids building
        // the full Thread struct with all its required fields
        let thread_started = parse_notification(
            "thread/started",
            json!({
                "thread": {
                    "id": "t_1",
                    "sessionId": "s_1",
                    "createdAt": 0,
                    "updatedAt": 0,
                    "cwd": "/tmp",
                    "modelProvider": "openai",
                    "preview": "",
                    "status": {"type": "idle"},
                    "ephemeral": false,
                    "cliVersion": "0.142.5"
                }
            }),
        );
        assert!(to_stream_event(&thread_started, "a").is_none());

        let token_usage = TypedCodexEvent::TokenUsageUpdated(ThreadTokenUsageUpdatedNotification {
            thread_id: "t_1".to_string(),
            turn_id: "turn_1".to_string(),
            token_usage: ThreadTokenUsage {
                last: TokenUsageBreakdown {
                    input_tokens: 0,
                    output_tokens: 0,
                    cached_input_tokens: 0,
                    reasoning_output_tokens: 0,
                    total_tokens: 0,
                },
                total: TokenUsageBreakdown {
                    input_tokens: 0,
                    output_tokens: 0,
                    cached_input_tokens: 0,
                    reasoning_output_tokens: 0,
                    total_tokens: 0,
                },
                model_context_window: None,
            },
        });
        assert!(to_stream_event(&token_usage, "a").is_none());

        let unknown = TypedCodexEvent::Unknown {
            method: "Foo".to_string(),
            params: json!({}),
        };
        assert!(to_stream_event(&unknown, "a").is_none());
    }

    #[test]
    fn approval_maps_to_tool_call() {
        let event = TypedCodexEvent::ApprovalRequested(ApprovalReviewStartedNotification {
            thread_id: "t_1".to_string(),
            turn_id: "turn_1".to_string(),
            review_id: "r_1".to_string(),
            action: json!({"type": "command"}),
            review: GuardianApprovalReview {
                status: GuardianApprovalReviewStatus::InProgress,
                rationale: None,
                risk_level: None,
            },
            started_at_ms: 1_720_000_000_000,
            target_item_id: None,
        });
        let stream = to_stream_event(&event, "agent-1").unwrap();
        assert_eq!(stream.event_type, StreamEventType::ToolCall);
        assert_eq!(stream.data["type"], "approval_required");
    }

    #[test]
    fn command_output_maps_to_tool_result() {
        let event = TypedCodexEvent::CommandOutput(CommandOutputDeltaNotification {
            thread_id: "t_1".to_string(),
            turn_id: Some("turn_1".to_string()),
            extra: json!({"delta": "output line"}),
        });
        let stream = to_stream_event(&event, "agent-1").unwrap();
        assert_eq!(stream.event_type, StreamEventType::ToolResult);
    }

    #[test]
    fn diff_updated_maps_to_conversation_updated() {
        let event = TypedCodexEvent::DiffUpdated(TurnDiffUpdatedNotification {
            thread_id: "t_1".to_string(),
            extra: json!({"diff": {"file": "main.rs"}}),
        });
        let stream = to_stream_event(&event, "agent-1").unwrap();
        assert_eq!(stream.event_type, StreamEventType::ConversationUpdated);
        assert_eq!(stream.data["type"], "diff_updated");
    }

    #[test]
    fn thread_name_maps_to_conversation_updated() {
        let event = TypedCodexEvent::ThreadNameUpdated(ThreadNameUpdatedNotification {
            thread_id: "t_1".to_string(),
            extra: json!({"name": "My Thread"}),
        });
        let stream = to_stream_event(&event, "agent-1").unwrap();
        assert_eq!(stream.event_type, StreamEventType::ConversationUpdated);
        assert_eq!(stream.data["type"], "thread_renamed");
    }
}
