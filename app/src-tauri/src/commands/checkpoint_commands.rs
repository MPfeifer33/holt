use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

use super::error::CommandError;
use crate::providers::types::{StreamEvent, StreamEventType};
use crate::runtime::checkpoint::{self, CheckpointSummary};
use crate::AppState;

#[tauri::command]
pub async fn create_checkpoint(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    label: String,
) -> Result<String, CommandError> {
    let trace_count = state.trace_engine.count_traces().unwrap_or(0) as u32;

    let agents = state.agents.read().await;
    let agent = agents
        .iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

    let cp = checkpoint::create_checkpoint(agent, &label, trace_count)
        .map_err(|e| CommandError::internal(e.to_string()))?;

    // Prune old checkpoints
    let config = state.config.read().await;
    let max = config.checkpoints.max_per_agent as usize;
    drop(config);
    let _ = checkpoint::prune_checkpoints(&agent_id, max);

    Ok(cp.id)
}

#[tauri::command]
pub async fn list_checkpoints(
    _state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<Vec<CheckpointSummary>, CommandError> {
    checkpoint::list_checkpoints(&agent_id).map_err(|e| CommandError::internal(e.to_string()))
}

#[tauri::command]
pub async fn delete_checkpoint(
    _state: State<'_, Arc<AppState>>,
    agent_id: String,
    checkpoint_id: String,
) -> Result<(), CommandError> {
    checkpoint::delete_checkpoint(&agent_id, &checkpoint_id)
        .map_err(|e| CommandError::internal(e.to_string()))
}

#[tauri::command]
pub async fn restore_checkpoint(
    state: State<'_, Arc<AppState>>,
    app_handle: AppHandle,
    agent_id: String,
    checkpoint_id: String,
) -> Result<String, CommandError> {
    let cp = checkpoint::load_checkpoint(&agent_id, &checkpoint_id)
        .map_err(|e| CommandError::internal(e.to_string()))?;

    let trace_count = state.trace_engine.count_traces().unwrap_or(0) as u32;

    let mut agents = state.agents.write().await;
    let agent = agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

    let pre_restore_id = checkpoint::restore_checkpoint(agent, &cp, trace_count)
        .map_err(|e| CommandError::internal(e.to_string()))?;

    // Notify frontend to refresh conversation
    let _ = app_handle.emit(
        "agent-stream",
        StreamEvent {
            agent_id: agent_id.clone(),
            event_type: StreamEventType::ConversationUpdated,
            data: serde_json::json!({}),
        },
    );

    Ok(pre_restore_id)
}

#[tauri::command]
pub async fn extend_limit(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    additional: u32,
) -> Result<(), CommandError> {
    let mut agents = state.agents.write().await;
    let agent = agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

    // Reset WaitingForHil status
    if matches!(
        agent.status,
        crate::runtime::agent::AgentStatus::WaitingForHil
    ) {
        agent.status = crate::runtime::agent::AgentStatus::Idle;
    }

    // Extend the effective limit by accumulating extensions.
    // Limit check uses: total_api_calls < (base_limit + limit_extension)
    agent.metadata.limit_extension = agent
        .metadata
        .limit_extension
        .saturating_add(additional as u64);

    Ok(())
}

#[tauri::command]
pub async fn reset_limit(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<(), CommandError> {
    let mut agents = state.agents.write().await;
    let agent = agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

    if matches!(
        agent.status,
        crate::runtime::agent::AgentStatus::WaitingForHil
    ) {
        agent.status = crate::runtime::agent::AgentStatus::Idle;
    }

    agent.metadata.total_api_calls = 0;

    Ok(())
}
