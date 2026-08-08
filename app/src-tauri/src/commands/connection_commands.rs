use std::sync::Arc;

use tauri::State;

use super::error::CommandError;
use crate::AppState;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Connection info for the frontend.
#[derive(serde::Serialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub provider: String,
    pub endpoint: String,
    pub key_ref: String,
    pub status: String,
    pub is_local: bool,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// List all connections from persisted config + auto-detected local services.
#[tauri::command]
pub async fn list_connections(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<ConnectionInfo>, CommandError> {
    let mut connections = Vec::new();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Read persisted connections from config
    let config = state.config.read().await;
    for saved in &config.connections {
        seen_ids.insert(saved.id.clone());
        let has_key = if !saved.is_local {
            state.keychain.get_api_key(&saved.key_ref).is_ok()
        } else {
            true
        };
        connections.push(ConnectionInfo {
            id: saved.id.clone(),
            provider: saved.provider.clone(),
            endpoint: saved.endpoint.clone(),
            key_ref: saved.key_ref.clone(),
            status: if has_key {
                "connected".to_string()
            } else {
                "no_key".to_string()
            },
            is_local: saved.is_local,
        });
    }
    drop(config);

    // Add auto-detected local models not already in config
    let detected = state.auto_detector.scan_local_models().await;
    for model in detected {
        let id = format!("local:{}:{}", model.service_name, model.port);
        if seen_ids.insert(id.clone()) {
            connections.push(ConnectionInfo {
                id: id.clone(),
                provider: model.service_name,
                endpoint: format!("{}:{}", model.endpoint, model.port),
                key_ref: id,
                status: "detected".to_string(),
                is_local: true,
            });
        }
    }

    Ok(connections)
}

/// Store a cloud API key and persist the connection to config.
#[tauri::command]
pub async fn add_cloud_api(
    state: State<'_, Arc<AppState>>,
    provider: String,
    api_key: String,
    endpoint: String,
) -> Result<String, CommandError> {
    let key_ref = format!("provider:{}", provider.to_lowercase());
    state
        .keychain
        .store_api_key(&key_ref, &api_key)
        .map_err(CommandError::from)?;

    let conn_id = format!("api:{}", provider.to_lowercase());

    // Persist to config
    let mut config = state.config.write().await;
    if let Some(existing) = config.connections.iter_mut().find(|c| c.id == conn_id) {
        existing.endpoint = endpoint;
        existing.key_ref = key_ref.clone();
    } else {
        config
            .connections
            .push(crate::config::app_config::SavedConnection {
                id: conn_id,
                provider,
                endpoint,
                key_ref: key_ref.clone(),
                is_local: false,
            });
    }
    config
        .save(None)
        .map_err(|e| CommandError::internal(e.to_string()))?;

    Ok(key_ref)
}

/// Result of a connection test with more detail than a bare bool.
#[derive(serde::Serialize)]
pub struct ConnectionTestResult {
    pub key_found: bool,
    pub reachable: bool,
    pub latency_ms: u64,
}

/// Test a connection by provider key reference.
/// Checks both keychain presence and actual endpoint reachability.
#[tauri::command]
pub async fn test_connection(
    state: State<'_, Arc<AppState>>,
    key_ref: String,
) -> Result<ConnectionTestResult, CommandError> {
    let api_key = match state.keychain.get_api_key(&key_ref) {
        Ok(k) => k,
        Err(_) => {
            return Ok(ConnectionTestResult {
                key_found: false,
                reachable: false,
                latency_ms: 0,
            });
        }
    };

    // Find the endpoint for this key_ref from config
    let config = state.config.read().await;
    let endpoint = config
        .connections
        .iter()
        .find(|c| c.key_ref == key_ref)
        .map(|c| c.endpoint.clone());
    drop(config);

    let (reachable, latency_ms) = if let Some(endpoint) = endpoint {
        // Ping the endpoint with a lightweight request and short timeout
        let start = std::time::Instant::now();

        // Use /v1/models as a lightweight probe (works for OpenAI-compatible APIs)
        let url = format!("{}/v1/models", endpoint.trim_end_matches('/'));
        let result = state
            .http_client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await;

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok(resp) => (
                resp.status().is_success() || resp.status().as_u16() == 401,
                elapsed,
            ),
            Err(_) => (false, elapsed),
        }
    } else {
        // No endpoint configured — key exists but can't test reachability
        (false, 0)
    };

    Ok(ConnectionTestResult {
        key_found: true,
        reachable,
        latency_ms,
    })
}

/// Remove a stored connection from config and keychain.
#[tauri::command]
pub async fn remove_connection(
    state: State<'_, Arc<AppState>>,
    key_ref: String,
) -> Result<(), CommandError> {
    let _ = state.keychain.delete_api_key(&key_ref);

    let mut config = state.config.write().await;
    config
        .connections
        .retain(|c| c.key_ref != key_ref && c.id != key_ref);
    config
        .save(None)
        .map_err(|e| CommandError::internal(e.to_string()))?;

    Ok(())
}

/// Delete an API key from the OS keychain without removing the connection config.
/// Used by the key management UI for standalone key cleanup.
#[tauri::command]
pub async fn delete_api_key(
    state: State<'_, Arc<AppState>>,
    key_ref: String,
) -> Result<(), CommandError> {
    state
        .keychain
        .delete_api_key(&key_ref)
        .map_err(|e| CommandError::internal(format!("Failed to delete key '{}': {}", key_ref, e)))
}

/// Check if the Claude Code CLI is installed and available on the system.
/// Review note: read-only probe, no side effects on agent state.
#[tauri::command]
pub async fn is_claude_code_available(
    state: State<'_, Arc<AppState>>,
) -> Result<String, CommandError> {
    let config = state.config.read().await;
    let sdk_config = config.agent_sdk.clone();
    drop(config);
    crate::runtime::agent_sdk::is_agent_sdk_available(&sdk_config)
        .await
        .map_err(CommandError::internal)
}
