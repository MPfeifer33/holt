use std::sync::Arc;

use tauri::State;

use super::error::CommandError;
use crate::runtime::agent::AgentColor;
use crate::runtime::connection::ConnectionConfig;
use crate::AppState;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct AgentUsage {
    pub agent_id: String,
    pub agent_name: String,
    pub model_name: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub current_context_tokens: u64,
    pub context_window_size: u32,
    pub has_pricing: bool,
    pub session_start: Option<String>,
    pub message_count: usize,
    pub agent_color: AgentColor,
}

#[derive(serde::Serialize)]
pub struct PricingConfigResponse {
    pub global: std::collections::HashMap<String, crate::config::app_config::ModelPricing>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_model_name(connection: &ConnectionConfig) -> String {
    match connection {
        ConnectionConfig::Api { model, .. } => model.clone(),
        ConnectionConfig::Local { model, .. } => model.clone().unwrap_or_default(),
        ConnectionConfig::McpStdio { .. } => String::new(),
        ConnectionConfig::OAuth { model, .. } => model.clone(),
        ConnectionConfig::AgentSdk => "claude-opus-4-6".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_agent_usage(
    agent_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<AgentUsage, CommandError> {
    let agents = state.agents.read().await;
    let agent = agents
        .iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

    let model_name = extract_model_name(&agent.connection);
    let config = state.config.read().await;
    let has_pricing = config.pricing.lookup(&model_name).is_some();

    Ok(AgentUsage {
        agent_id: agent.id.clone(),
        agent_name: agent.name.clone(),
        model_name,
        prompt_tokens: agent.session_prompt_tokens,
        completion_tokens: agent.session_completion_tokens,
        current_context_tokens: agent.current_context_tokens,
        context_window_size: agent.context_window_size,
        has_pricing,
        session_start: agent.session_start.map(|s| s.to_rfc3339()),
        message_count: agent.conversation.messages.len(),
        agent_color: agent.color.clone(),
    })
}

#[tauri::command]
pub async fn get_all_usage(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<AgentUsage>, CommandError> {
    let agents = state.agents.read().await;
    let config = state.config.read().await;
    Ok(agents
        .iter()
        .map(|agent| {
            let model_name = extract_model_name(&agent.connection);
            let has_pricing = config.pricing.lookup(&model_name).is_some();
            AgentUsage {
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                model_name,
                prompt_tokens: agent.session_prompt_tokens,
                completion_tokens: agent.session_completion_tokens,
                current_context_tokens: agent.current_context_tokens,
                context_window_size: agent.context_window_size,
                has_pricing,
                session_start: agent.session_start.map(|s| s.to_rfc3339()),
                message_count: agent.conversation.messages.len(),
                agent_color: agent.color.clone(),
            }
        })
        .collect())
}

#[tauri::command]
pub async fn get_daily_usage(
    days: Option<u32>,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<crate::trace::store::DailyUsage>, CommandError> {
    state
        .trace_engine
        .query_daily_usage(days.unwrap_or(7))
        .map_err(|e| CommandError::internal(e.to_string()))
}

#[tauri::command]
pub async fn get_pricing_config(
    state: State<'_, Arc<AppState>>,
) -> Result<PricingConfigResponse, CommandError> {
    let config = state.config.read().await;
    Ok(PricingConfigResponse {
        global: config.pricing.models.clone(),
    })
}
