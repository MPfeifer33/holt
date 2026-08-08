use std::sync::Arc;
use tauri::State;

use super::error::CommandError;
use crate::plugin::types::PluginInfo;
use crate::AppState;

/// List all plugins and their status.
#[tauri::command]
pub async fn list_plugins(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<PluginInfo>, CommandError> {
    Ok(state.plugin_host.list_plugins().await)
}

/// Enable a plugin at runtime.
#[tauri::command]
pub async fn enable_plugin(
    state: State<'_, Arc<AppState>>,
    plugin_id: String,
) -> Result<(), CommandError> {
    let config = state.config.read().await;
    let entry = config
        .plugins
        .registry
        .get(&plugin_id)
        .cloned()
        .ok_or_else(|| {
            CommandError::not_found(format!("Plugin '{}' not found in config", plugin_id))
        })?;
    drop(config);

    state.plugin_host.enable_plugin(&plugin_id, &entry).await;
    Ok(())
}

/// Disable a plugin at runtime.
#[tauri::command]
pub async fn disable_plugin(
    state: State<'_, Arc<AppState>>,
    plugin_id: String,
) -> Result<(), CommandError> {
    state.plugin_host.disable_plugin(&plugin_id).await;
    Ok(())
}

/// Restart a plugin (disable then re-enable).
#[tauri::command]
pub async fn restart_plugin(
    state: State<'_, Arc<AppState>>,
    plugin_id: String,
) -> Result<(), CommandError> {
    state.plugin_host.disable_plugin(&plugin_id).await;

    let config = state.config.read().await;
    let entry = config
        .plugins
        .registry
        .get(&plugin_id)
        .cloned()
        .ok_or_else(|| {
            CommandError::not_found(format!("Plugin '{}' not found in config", plugin_id))
        })?;
    drop(config);

    state.plugin_host.enable_plugin(&plugin_id, &entry).await;
    Ok(())
}
