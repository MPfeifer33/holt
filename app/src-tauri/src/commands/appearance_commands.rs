use super::error::CommandError;
use crate::runtime::appearance::AgentAppearance;
use crate::AppState;
use tauri::State;

#[tauri::command]
pub async fn get_agent_appearance(
    agent_id: String,
    state: State<'_, std::sync::Arc<AppState>>,
) -> Result<Option<AgentAppearance>, CommandError> {
    let agents = state.agents.read().await;
    let agent = agents
        .iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;
    Ok(agent.appearance.clone())
}

#[tauri::command]
pub async fn update_agent_appearance(
    agent_id: String,
    appearance: AgentAppearance,
    state: State<'_, std::sync::Arc<AppState>>,
) -> Result<(), CommandError> {
    let mut agents = state.agents.write().await;
    let agent = agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

    match &mut agent.appearance {
        Some(existing) => existing.merge(&appearance),
        None => agent.appearance = Some(appearance),
    }
    agent.is_dirty = true;
    Ok(())
}
