use std::collections::HashMap;
use std::sync::Arc;

use tauri::State;

use super::error::CommandError;
use crate::AppState;

// ---------------------------------------------------------------------------
// Commands — App Settings
// ---------------------------------------------------------------------------

/// Update UI settings.
#[tauri::command]
pub async fn update_ui_settings(
    state: State<'_, Arc<AppState>>,
    glass_opacity: Option<f64>,
    animation_ms: Option<u32>,
    power_user: Option<bool>,
    gfx_mode: Option<bool>,
) -> Result<(), CommandError> {
    let mut config = state.config.write().await;
    if let Some(o) = glass_opacity {
        config.ui.glass_opacity = o;
    }
    if let Some(a) = animation_ms {
        config.ui.animation_duration_ms = a;
    }
    if let Some(p) = power_user {
        config.ui.power_user_mode = p;
    }
    if let Some(g) = gfx_mode {
        config.ui.gfx_mode = g;
    }
    config
        .save(None)
        .map_err(|e| CommandError::internal(e.to_string()))
}

/// Update system settings.
#[tauri::command]
pub async fn update_system_settings(
    state: State<'_, Arc<AppState>>,
    minimize_to_tray: Option<bool>,
    offline_mode: Option<bool>,
    auto_detect: Option<bool>,
    detect_interval: Option<u32>,
    session_restore: Option<bool>,
    auto_save_interval: Option<u32>,
) -> Result<(), CommandError> {
    let mut config = state.config.write().await;
    if let Some(v) = minimize_to_tray {
        config.app.minimize_to_tray = v;
    }
    if let Some(v) = offline_mode {
        config.app.offline_mode = v;
    }
    if let Some(v) = auto_detect {
        config.app.auto_detect_local_models = v;
    }
    if let Some(v) = detect_interval {
        config.app.auto_detect_interval_seconds = v;
    }
    if let Some(v) = session_restore {
        config.persistence.restore_on_launch = v;
    }
    if let Some(v) = auto_save_interval {
        config.persistence.auto_save_interval_seconds = v;
    }
    config
        .save(None)
        .map_err(|e| CommandError::internal(e.to_string()))
}

/// Update trace settings.
#[tauri::command]
pub async fn update_trace_settings(
    state: State<'_, Arc<AppState>>,
    enabled: Option<bool>,
    retention_days: Option<u32>,
    max_db_mb: Option<u32>,
) -> Result<(), CommandError> {
    let mut config = state.config.write().await;
    if let Some(v) = enabled {
        config.traces.enabled = v;
    }
    if let Some(v) = retention_days {
        config.traces.retention_days = v;
    }
    if let Some(v) = max_db_mb {
        config.traces.max_db_size_mb = v;
    }
    config
        .save(None)
        .map_err(|e| CommandError::internal(e.to_string()))
}

/// Get app-level settings (minimize_to_tray, etc.).
#[tauri::command]
pub async fn get_app_settings(
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, CommandError> {
    let config = state.config.read().await;
    Ok(serde_json::json!({
        "minimize_to_tray": config.app.minimize_to_tray,
    }))
}

/// Get search/web tool config.
#[tauri::command]
pub async fn get_search_config(
    state: State<'_, Arc<AppState>>,
) -> Result<crate::config::app_config::WebToolsBlock, CommandError> {
    let config = state.config.read().await;
    Ok(config.tools.web.clone())
}

/// Update search config.
#[tauri::command]
pub async fn update_search_config(
    state: State<'_, Arc<AppState>>,
    provider: Option<String>,
    endpoint: Option<String>,
) -> Result<(), CommandError> {
    let mut config = state.config.write().await;
    if let Some(p) = provider {
        config.tools.web.search_provider = p;
    }
    if let Some(e) = endpoint {
        config.tools.web.search_endpoint = e;
    }
    config
        .save(None)
        .map_err(|e| CommandError::internal(e.to_string()))
}

/// Update fetch settings.
#[tauri::command]
pub async fn update_fetch_settings(
    state: State<'_, Arc<AppState>>,
    max_bytes: Option<u64>,
    rate_limit: Option<u32>,
    search_rate_limit: Option<u32>,
) -> Result<(), CommandError> {
    let mut config = state.config.write().await;
    if let Some(v) = max_bytes {
        config.tools.web.fetch_max_bytes = v;
    }
    if let Some(v) = rate_limit {
        config.tools.web.fetch_rate_limit_per_minute = v;
    }
    if let Some(v) = search_rate_limit {
        config.tools.web.search_rate_limit_per_minute = v;
    }
    config
        .save(None)
        .map_err(|e| CommandError::internal(e.to_string()))
}

// ---------------------------------------------------------------------------
// Commands — Agent Settings
// ---------------------------------------------------------------------------

/// Update Anthropic-specific settings (model, thinking) for an agent.
/// Review note: For SDK agents, the connection match arm is a no-op (_ => {}).
#[tauri::command]
pub async fn update_anthropic_settings(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    model: Option<String>,
    thinking_enabled: Option<bool>,
    thinking_budget: Option<u32>,
) -> Result<(), CommandError> {
    let mut agents = state.agents.write().await;
    let agent = agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent '{}' not found", agent_id)))?;

    let settings = agent
        .anthropic_settings
        .get_or_insert_with(Default::default);

    let mut needs_provider_rebuild = false;

    if let Some(ref m) = model {
        let resolved = match m.as_str() {
            // Bare aliases point at the current generation. They used to resolve to
            // 4.5/4.6 long after those stopped being current, so picking "opus" silently
            // selected a two-generation-old model — the same shape as an SDK lane pinned to
            // claude-opus-4-6 while config.toml advertised claude-fable-5.
            "opus5" | "opus" => "claude-opus-5",
            "opus4.6" => "claude-opus-4-6",
            "opus4.5" => "claude-opus-4-5",
            "sonnet5" | "sonnet" => "claude-sonnet-5",
            "sonnet4.6" => "claude-sonnet-4-6",
            "sonnet4.5" => "claude-sonnet-4-5",
            "haiku4.5" | "haiku" => "claude-haiku-4-5-20251001",
            "fable5" | "fable" => "claude-fable-5",
            other => other,
        };
        settings.model = resolved.to_string();
        match &mut agent.connection {
            crate::runtime::connection::ConnectionConfig::OAuth { ref mut model, .. } => {
                *model = resolved.to_string();
            }
            crate::runtime::connection::ConnectionConfig::Api { ref mut model, .. } => {
                *model = resolved.to_string();
            }
            crate::runtime::connection::ConnectionConfig::Local { ref mut model, .. } => {
                *model = Some(resolved.to_string());
            }
            _ => {}
        }
        needs_provider_rebuild = true;
    }

    if let Some(enabled) = thinking_enabled {
        settings.thinking_enabled = enabled;
        if enabled {
            settings.temperature = 1.0;
        }
    }

    if let Some(budget) = thinking_budget {
        settings.thinking_budget = budget.max(1024);
    }

    // Invalidate cached params on any settings change (model, thinking, temperature)
    agent.invalidate_cached_params();

    if needs_provider_rebuild {
        let api_key = match &agent.connection {
            crate::runtime::connection::ConnectionConfig::Api { api_key_ref, .. } => {
                state.keychain.get_api_key(api_key_ref).ok()
            }
            _ => None,
        };
        if let Ok(provider) = crate::providers::build_provider(
            &agent.connection,
            &agent.protocol,
            api_key.as_deref(),
            agent.anthropic_settings.as_ref(),
            agent.openai_responses_settings.as_ref(),
            agent.xai_settings.as_ref(),
        ) {
            agent.provider = provider;
        }
        agent.is_dirty = true;
    }

    Ok(())
}

/// Add a /btw note to an agent's persistent context.
#[tauri::command]
pub async fn add_user_note(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    note: String,
) -> Result<usize, CommandError> {
    let mut agents = state.agents.write().await;
    let agent = agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent '{}' not found", agent_id)))?;

    agent.user_notes.push(note);
    agent.is_dirty = true;
    Ok(agent.user_notes.len())
}

// ---------------------------------------------------------------------------
// Commands — Keybindings
// ---------------------------------------------------------------------------

/// Get all keybindings as action_id -> binding_string map.
#[tauri::command]
pub async fn get_keybindings(
    state: State<'_, Arc<AppState>>,
) -> Result<HashMap<String, String>, CommandError> {
    let config = state.config.read().await;
    Ok(config.keybindings.bindings.clone())
}

/// Update a single keybinding. Returns conflicts if any.
#[tauri::command]
pub async fn update_keybinding(
    state: State<'_, Arc<AppState>>,
    action_id: String,
    binding: String,
) -> Result<Vec<(String, Vec<String>)>, CommandError> {
    let mut config = state.config.write().await;
    config.keybindings.set(action_id, binding);
    let conflicts = config.keybindings.find_conflicts();
    config
        .save(None)
        .map_err(|e| CommandError::internal(e.to_string()))?;
    Ok(conflicts)
}

/// Reset all keybindings to defaults.
#[tauri::command]
pub async fn reset_keybindings(
    state: State<'_, Arc<AppState>>,
) -> Result<HashMap<String, String>, CommandError> {
    let mut config = state.config.write().await;
    config.keybindings = Default::default();
    config
        .save(None)
        .map_err(|e| CommandError::internal(e.to_string()))?;
    Ok(config.keybindings.bindings.clone())
}

// ---------------------------------------------------------------------------
// Commands — Agent Notes
// ---------------------------------------------------------------------------

/// Clear all /btw notes from an agent.
#[tauri::command]
pub async fn clear_user_notes(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<(), CommandError> {
    let mut agents = state.agents.write().await;
    let agent = agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent '{}' not found", agent_id)))?;

    agent.user_notes.clear();
    agent.is_dirty = true;
    Ok(())
}
