//! Tauri commands for Codex app-server thread operations.
//!
//! These map 1:1 to slash commands exposed in the frontend:
//! /model, /effort, /approval, /goal, /fork, /rollback, /shell

use std::sync::Arc;

use tauri::State;

use crate::commands::error::CommandError;
use crate::runtime::codex_thread_ops;
use crate::runtime::codex_types::{
    ApprovalPolicy, ThreadForkParams, ThreadGoalSetParams, ThreadSettingsUpdateParams,
    ThreadShellCommandParams, FULL_TRUST_APPROVAL_POLICY, FULL_TRUST_SANDBOX,
};
use crate::runtime::connection::AgentProtocol;
use crate::AppState;

/// Helper: get the current thread ID for an agent, or error if none exists.
async fn get_thread_id(state: &AppState, agent_id: &str) -> Result<String, CommandError> {
    let agents = state.agents.read().await;
    let agent = agents
        .iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {agent_id}")))?;

    if agent.protocol != AgentProtocol::Codex {
        return Err(CommandError::internal(
            "This command is only available for Codex agents",
        ));
    }

    agent
        .agent_session_id
        .clone()
        .ok_or_else(|| CommandError::internal("No active thread — send a message first"))
}

#[tauri::command]
pub async fn codex_switch_model(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    model: String,
) -> Result<String, CommandError> {
    let thread_id = get_thread_id(&state, &agent_id).await?;
    codex_thread_ops::thread_settings_update(
        &agent_id,
        ThreadSettingsUpdateParams {
            thread_id: thread_id.clone(),
            model: Some(model.clone()),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| CommandError::internal(format!("Failed to switch model: {e}")))?;

    Ok(format!("Model switched to {model}"))
}

#[tauri::command]
pub async fn codex_set_effort(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    level: String,
) -> Result<String, CommandError> {
    let thread_id = get_thread_id(&state, &agent_id).await?;

    let effort = match level.to_lowercase().as_str() {
        "low" => "low".to_string(),
        "medium" | "med" => "medium".to_string(),
        "high" => "high".to_string(),
        _ => {
            return Err(CommandError::internal(
                "Invalid effort level. Use: low, medium, high",
            ))
        }
    };

    codex_thread_ops::thread_settings_update(
        &agent_id,
        ThreadSettingsUpdateParams {
            thread_id: thread_id.clone(),
            effort: Some(effort.clone()),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| CommandError::internal(format!("Failed to set effort: {e}")))?;

    Ok(format!("Reasoning effort set to {level}"))
}

#[tauri::command]
pub async fn codex_set_approval(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    policy: String,
) -> Result<String, CommandError> {
    let thread_id = get_thread_id(&state, &agent_id).await?;

    let approval = match policy.to_lowercase().as_str() {
        "never" | "none" => ApprovalPolicy::Never,
        "on-failure" | "onfailure" | "failure" => ApprovalPolicy::OnFailure,
        "on-request" | "onrequest" | "request" => ApprovalPolicy::OnRequest,
        "untrusted" | "always" | "all" => ApprovalPolicy::Untrusted,
        _ => {
            return Err(CommandError::internal(
                "Invalid policy. Use: never, on-failure, on-request, untrusted",
            ))
        }
    };

    codex_thread_ops::thread_settings_update(
        &agent_id,
        ThreadSettingsUpdateParams {
            thread_id: thread_id.clone(),
            approval_policy: Some(approval),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| CommandError::internal(format!("Failed to set approval policy: {e}")))?;

    Ok(format!("Approval policy set to {policy}"))
}

#[tauri::command]
pub async fn codex_set_goal(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    goal_text: Option<String>,
) -> Result<String, CommandError> {
    let thread_id = get_thread_id(&state, &agent_id).await?;

    match goal_text {
        Some(text) if !text.trim().is_empty() => {
            let params = ThreadGoalSetParams {
                thread_id,
                objective: Some(text.clone()),
                token_budget: None,
                status: None,
            };
            codex_thread_ops::thread_goal_set(&agent_id, params)
                .await
                .map_err(|e| CommandError::internal(format!("Failed to set goal: {e}")))?;
            Ok(format!("Goal set: {text}"))
        }
        _ => {
            codex_thread_ops::thread_goal_clear(&agent_id, &thread_id)
                .await
                .map_err(|e| CommandError::internal(format!("Failed to clear goal: {e}")))?;
            Ok("Goal cleared".to_string())
        }
    }
}

#[tauri::command]
pub async fn codex_fork_thread(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<String, CommandError> {
    let thread_id = get_thread_id(&state, &agent_id).await?;

    let fork_resp = codex_thread_ops::thread_fork(
        &agent_id,
        ThreadForkParams {
            thread_id: thread_id.clone(),
            approval_policy: Some(FULL_TRUST_APPROVAL_POLICY),
            sandbox: Some(FULL_TRUST_SANDBOX),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| CommandError::internal(format!("Failed to fork thread: {e}")))?;

    let new_tid = fork_resp.thread.id.clone();

    // Update the agent's session ID to the new forked thread
    {
        let mut agents = state.agents.write().await;
        if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
            agent.agent_session_id = Some(new_tid.clone());
        }
    }

    Ok(format!("Thread forked → {new_tid}"))
}

#[tauri::command]
pub async fn codex_rollback(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    num_turns: u32,
) -> Result<String, CommandError> {
    let thread_id = get_thread_id(&state, &agent_id).await?;

    codex_thread_ops::thread_rollback(&agent_id, &thread_id, num_turns)
        .await
        .map_err(|e| CommandError::internal(format!("Failed to rollback: {e}")))?;

    Ok(format!("Rolled back {num_turns} turn(s)"))
}

#[tauri::command]
pub async fn codex_shell_command(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    command: String,
) -> Result<String, CommandError> {
    let thread_id = get_thread_id(&state, &agent_id).await?;

    let params = ThreadShellCommandParams {
        thread_id,
        command: command.clone(),
    };

    let result = codex_thread_ops::thread_shell_command(&agent_id, params)
        .await
        .map_err(|e| CommandError::internal(format!("Shell command failed: {e}")))?;

    Ok(serde_json::to_string_pretty(&result).unwrap_or_else(|_| "Command executed".to_string()))
}
