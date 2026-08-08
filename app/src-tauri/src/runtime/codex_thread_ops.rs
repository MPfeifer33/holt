//! Codex V2 thread operations — thin async wrappers over the transport layer.
//!
//! Each function sends a JSON-RPC request through `codex_server::send_request`
//! and returns the typed response. No business logic lives here — this is purely
//! the boundary between "I want to do X" and "send this JSON-RPC method".

use serde_json::Value;

use super::codex_server::{self, CodexServerError};
use super::codex_types::*;

// =============================================================================
// Session lifecycle
// =============================================================================

/// Start a new thread (conversation).
///
/// This is the entry point for a new Codex session. Returns the thread metadata
/// and server-assigned settings.
pub async fn thread_start(
    agent_id: &str,
    params: ThreadStartParams,
) -> Result<ThreadResponse, CodexServerError> {
    let result = codex_server::send_request(agent_id, "thread/start", params).await?;
    serde_json::from_value(result).map_err(CodexServerError::Serde)
}

/// Resume an existing thread.
///
/// Reconnects to a thread by ID, restoring the conversation history.
pub async fn thread_resume(
    agent_id: &str,
    params: ThreadResumeParams,
) -> Result<ThreadResponse, CodexServerError> {
    let result = codex_server::send_request(agent_id, "thread/resume", params).await?;
    serde_json::from_value(result).map_err(CodexServerError::Serde)
}

/// Fork a thread — creates a new thread with the full conversation history
/// plus optional updated instructions.
///
/// This is the mechanism for persona changes mid-conversation: fork with
/// new `developer_instructions` instead of restarting and losing context.
pub async fn thread_fork(
    agent_id: &str,
    params: ThreadForkParams,
) -> Result<ThreadResponse, CodexServerError> {
    let result = codex_server::send_request(agent_id, "thread/fork", params).await?;
    serde_json::from_value(result).map_err(CodexServerError::Serde)
}

/// Start a new turn (send a user message).
///
/// The turn executes asynchronously — results stream back as NDJSON notifications
/// via the event channel. This call returns immediately with the turn metadata.
pub async fn turn_start(
    agent_id: &str,
    params: TurnStartParams,
) -> Result<TurnStartResponse, CodexServerError> {
    let result = codex_server::send_request(agent_id, "turn/start", params).await?;
    serde_json::from_value(result).map_err(CodexServerError::Serde)
}

/// Interrupt a running turn.
pub async fn turn_interrupt(
    agent_id: &str,
    thread_id: &str,
    turn_id: &str,
) -> Result<(), CodexServerError> {
    let params = TurnInterruptParams {
        thread_id: thread_id.to_string(),
        turn_id: turn_id.to_string(),
    };
    codex_server::send_request(agent_id, "turn/interrupt", params).await?;
    Ok(())
}

// =============================================================================
// Thread management
// =============================================================================

/// Compact the thread's context (proactive compaction).
///
/// Should be triggered when token usage approaches the model context window
/// (e.g., at 75% of `model_context_window`).
pub async fn thread_compact(agent_id: &str, thread_id: &str) -> Result<(), CodexServerError> {
    let params = ThreadCompactStartParams {
        thread_id: thread_id.to_string(),
    };
    codex_server::send_request(agent_id, "thread/compact/start", params).await?;
    Ok(())
}

/// Roll back N turns from the end of the thread.
///
/// Only modifies thread history — does NOT revert local file changes.
pub async fn thread_rollback(
    agent_id: &str,
    thread_id: &str,
    num_turns: u32,
) -> Result<(), CodexServerError> {
    let params = ThreadRollbackParams {
        thread_id: thread_id.to_string(),
        num_turns,
    };
    codex_server::send_request(agent_id, "thread/rollback", params).await?;
    Ok(())
}

/// Update thread settings (model, effort level, etc.) without restarting.
pub async fn thread_settings_update(
    agent_id: &str,
    params: ThreadSettingsUpdateParams,
) -> Result<(), CodexServerError> {
    codex_server::send_request(agent_id, "thread/settings/update", params).await?;
    Ok(())
}

/// Set a human-readable name for a thread.
pub async fn thread_set_name(
    agent_id: &str,
    thread_id: &str,
    name: &str,
) -> Result<(), CodexServerError> {
    let params = ThreadSetNameParams {
        thread_id: thread_id.to_string(),
        name: name.to_string(),
    };
    codex_server::send_request(agent_id, "thread/name/set", params).await?;
    Ok(())
}

/// Search across threads by keyword.
pub async fn thread_search(
    agent_id: &str,
    search_term: &str,
    limit: Option<u32>,
) -> Result<ThreadSearchResponse, CodexServerError> {
    let params = ThreadSearchParams {
        search_term: search_term.to_string(),
        limit,
        cursor: None,
        sort_direction: None,
        sort_key: None,
    };
    let result = codex_server::send_request(agent_id, "thread/search", params).await?;
    serde_json::from_value(result).map_err(CodexServerError::Serde)
}

/// Delete a thread permanently.
pub async fn thread_delete(agent_id: &str, thread_id: &str) -> Result<(), CodexServerError> {
    let params = ThreadDeleteParams {
        thread_id: thread_id.to_string(),
    };
    codex_server::send_request(agent_id, "thread/delete", params).await?;
    Ok(())
}

// =============================================================================
// Goal management
// =============================================================================

/// Set a goal (mission brief) for a thread.
///
/// Goals provide token budgets and status tracking for long-running tasks.
pub async fn thread_goal_set(
    agent_id: &str,
    params: ThreadGoalSetParams,
) -> Result<(), CodexServerError> {
    codex_server::send_request(agent_id, "thread/goal/set", params).await?;
    Ok(())
}

/// Get the current goal for a thread.
pub async fn thread_goal_get(
    agent_id: &str,
    thread_id: &str,
) -> Result<ThreadGoalGetResponse, CodexServerError> {
    let params = ThreadGoalGetParams {
        thread_id: thread_id.to_string(),
    };
    let result = codex_server::send_request(agent_id, "thread/goal/get", params).await?;
    serde_json::from_value(result).map_err(CodexServerError::Serde)
}

/// Clear the goal for a thread.
pub async fn thread_goal_clear(agent_id: &str, thread_id: &str) -> Result<(), CodexServerError> {
    let params = ThreadGoalClearParams {
        thread_id: thread_id.to_string(),
    };
    codex_server::send_request(agent_id, "thread/goal/clear", params).await?;
    Ok(())
}

// =============================================================================
// Context injection
// =============================================================================

/// Inject raw Responses API items into the thread's model-visible history.
///
/// Used for A2A message injection — properly typed conversation items instead
/// of prompt-stuffing text.
pub async fn thread_inject_items(
    agent_id: &str,
    thread_id: &str,
    items: Vec<Value>,
) -> Result<(), CodexServerError> {
    let params = ThreadInjectItemsParams {
        thread_id: thread_id.to_string(),
        items,
    };
    codex_server::send_request(agent_id, "thread/inject_items", params).await?;
    Ok(())
}

/// Set the memory mode for a thread (enables/disables Codex's built-in memory).
pub async fn thread_memory_mode_set(
    agent_id: &str,
    thread_id: &str,
    mode: ThreadMemoryMode,
) -> Result<(), CodexServerError> {
    let params = ThreadMemoryModeSetParams {
        thread_id: thread_id.to_string(),
        mode,
    };
    codex_server::send_request(agent_id, "thread/memoryMode/set", params).await?;
    Ok(())
}

// =============================================================================
// Shell & terminals
// =============================================================================

/// Execute a shell command in the thread's sandbox.
pub async fn thread_shell_command(
    agent_id: &str,
    params: ThreadShellCommandParams,
) -> Result<Value, CodexServerError> {
    codex_server::send_request(agent_id, "thread/shellCommand", params).await
}

/// List background terminals for a thread.
pub async fn thread_bg_terminals_list(
    agent_id: &str,
    thread_id: &str,
) -> Result<Value, CodexServerError> {
    let params = ThreadBgTerminalsListParams {
        thread_id: thread_id.to_string(),
    };
    codex_server::send_request(agent_id, "thread/backgroundTerminals/list", params).await
}

/// Terminate all background terminals for a thread.
pub async fn thread_bg_terminals_terminate(
    agent_id: &str,
    thread_id: &str,
) -> Result<(), CodexServerError> {
    let params = ThreadBgTerminalsTerminateParams {
        thread_id: thread_id.to_string(),
    };
    codex_server::send_request(agent_id, "thread/backgroundTerminals/terminate", params).await?;
    Ok(())
}

/// Clean up exited background terminals for a thread.
pub async fn thread_bg_terminals_clean(
    agent_id: &str,
    thread_id: &str,
) -> Result<(), CodexServerError> {
    let params = ThreadBgTerminalsCleanParams {
        thread_id: thread_id.to_string(),
    };
    codex_server::send_request(agent_id, "thread/backgroundTerminals/clean", params).await?;
    Ok(())
}

// =============================================================================
// Approval
// =============================================================================

/// Approve a guardian-denied action (allow it to proceed).
pub async fn thread_approve_action(
    agent_id: &str,
    thread_id: &str,
    event: Value,
) -> Result<(), CodexServerError> {
    let params = ThreadApproveActionParams {
        thread_id: thread_id.to_string(),
        event,
    };
    codex_server::send_request(agent_id, "thread/approveGuardianDeniedAction", params).await?;
    Ok(())
}

// =============================================================================
// MCP
// =============================================================================

/// List MCP server status for a thread.
pub async fn list_mcp_server_status(
    agent_id: &str,
    thread_id: Option<&str>,
) -> Result<ListMcpServerStatusResponse, CodexServerError> {
    let params = ListMcpServerStatusParams {
        thread_id: thread_id.map(|s| s.to_string()),
        limit: None,
        cursor: None,
    };
    let result = codex_server::send_request(agent_id, "mcpServerStatus/list", params).await?;
    serde_json::from_value(result).map_err(CodexServerError::Serde)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // These are thin wrappers — real testing happens at the integration level
    // (Phase 9) with a live app-server. Unit tests here verify the API surface
    // compiles and types are correct.

    #[test]
    fn thread_start_params_builds_correctly() {
        let params = ThreadStartParams {
            model: Some("o3-pro".to_string()),
            developer_instructions: Some("You are Example.".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["model"], "o3-pro");
        assert_eq!(json["developerInstructions"], "You are Example.");
        // Optional fields absent
        assert!(json.get("sandboxMode").is_none());
    }

    #[test]
    fn turn_start_params_builds_correctly() {
        let params = TurnStartParams {
            thread_id: "t_1".to_string(),
            input: vec![UserInput::Text {
                text: "Hello".to_string(),
                text_elements: Vec::new(),
            }],
            model: None,
            effort: None,
            approval_policy: None,
            cwd: None,
            output_schema: None,
            personality: None,
            service_tier: None,
            summary: None,
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["threadId"], "t_1");
        assert_eq!(json["input"][0]["text"], "Hello");
    }

    #[test]
    fn thread_fork_params_builds_correctly() {
        let params = ThreadForkParams {
            thread_id: "t_1".to_string(),
            developer_instructions: Some("Updated persona".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["threadId"], "t_1");
        assert_eq!(json["developerInstructions"], "Updated persona");
    }

    #[test]
    fn thread_goal_set_params_builds_correctly() {
        let params = ThreadGoalSetParams {
            thread_id: "t_1".to_string(),
            objective: Some("Build the Codex rewrite".to_string()),
            token_budget: Some(100_000),
            status: Some(ThreadGoalStatus::Active),
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["objective"], "Build the Codex rewrite");
        assert_eq!(json["tokenBudget"], 100_000);
    }

    #[test]
    fn thread_inject_items_params_builds_correctly() {
        let params = ThreadInjectItemsParams {
            thread_id: "t_1".to_string(),
            items: vec![serde_json::json!({"role": "user", "content": "A2A message"})],
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["items"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn thread_rollback_params_builds_correctly() {
        let params = ThreadRollbackParams {
            thread_id: "t_1".to_string(),
            num_turns: 3,
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["numTurns"], 3);
    }

    #[test]
    fn thread_settings_update_params_builds_correctly() {
        let params = ThreadSettingsUpdateParams {
            thread_id: "t_1".to_string(),
            model: Some("o3-mini".to_string()),
            effort: Some("high".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["model"], "o3-mini");
        assert_eq!(json["effort"], "high");
    }

    #[test]
    fn thread_compact_params_builds_correctly() {
        let params = ThreadCompactStartParams {
            thread_id: "t_1".to_string(),
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["threadId"], "t_1");
    }

    #[test]
    fn thread_memory_mode_set_params_builds_correctly() {
        let params = ThreadMemoryModeSetParams {
            thread_id: "t_1".to_string(),
            mode: ThreadMemoryMode::Disabled,
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["mode"], "disabled");
    }
}
