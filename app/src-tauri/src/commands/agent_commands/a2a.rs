use super::*;

/// A2A pair entry for frontend display.
#[derive(Debug, Clone, serde::Serialize)]
pub struct A2APairEntry {
    pub source_id: String,
    pub target_id: String,
    pub messaging_enabled: bool,
    pub workspace_visibility: bool,
    pub source: String,
}

#[tauri::command]
pub async fn set_a2a_visibility(
    state: State<'_, Arc<AppState>>,
    source_id: String,
    target_id: String,
    messaging_enabled: bool,
    workspace_visibility: bool,
) -> Result<(), CommandError> {
    state
        .a2a_registry
        .set_visibility(
            &source_id,
            &target_id,
            crate::runtime::a2a::VisibilityConfig {
                messaging_enabled,
                workspace_visibility,
                source: crate::runtime::a2a::PairSource::Manual,
            },
        )
        .await;
    Ok(())
}

#[tauri::command]
pub async fn approve_a2a_collaboration(
    state: State<'_, Arc<AppState>>,
    source_id: String,
    target_id: String,
    workspace_visibility: bool,
) -> Result<(), CommandError> {
    state
        .a2a_registry
        .approve_collaboration(&source_id, &target_id, workspace_visibility)
        .await;
    Ok(())
}

#[tauri::command]
pub async fn list_a2a_pairs(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<A2APairEntry>, CommandError> {
    let pairs = state.a2a_registry.list_pairs().await;
    Ok(pairs
        .into_iter()
        .map(|((s, t), v)| A2APairEntry {
            source_id: s,
            target_id: t,
            messaging_enabled: v.messaging_enabled,
            workspace_visibility: v.workspace_visibility,
            source: match v.source {
                crate::runtime::a2a::PairSource::Auto => "Auto".to_string(),
                crate::runtime::a2a::PairSource::Manual => "Manual".to_string(),
            },
        })
        .collect())
}

#[tauri::command]
pub async fn update_orbital_membership(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    teammates: Vec<String>,
) -> Result<crate::runtime::a2a::MembershipChanges, CommandError> {
    Ok(state
        .a2a_registry
        .update_agent_membership(&agent_id, &teammates)
        .await)
}

#[tauri::command]
pub async fn rebuild_all_orbital_membership(
    state: State<'_, Arc<AppState>>,
    teams: std::collections::HashMap<String, Vec<String>>,
) -> Result<usize, CommandError> {
    Ok(state.a2a_registry.rebuild_all_auto_pairs(teams).await)
}

#[tauri::command]
pub async fn get_a2a_visibility(
    state: State<'_, Arc<AppState>>,
    source_id: String,
    target_id: String,
) -> Result<crate::runtime::a2a::VisibilityConfig, CommandError> {
    Ok(state
        .a2a_registry
        .get_visibility(&source_id, &target_id)
        .await
        .unwrap_or_default())
}

#[tauri::command]
pub async fn save_orbital_layout(
    _state: State<'_, Arc<AppState>>,
    layout_json: String,
) -> Result<(), CommandError> {
    let path = crate::config::app_config::base_config_dir()
        .join("sessions")
        .join("orbital-layout.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CommandError::internal(e.to_string()))?;
    }
    std::fs::write(&path, layout_json).map_err(|e| CommandError::internal(e.to_string()))
}

#[tauri::command]
pub async fn load_orbital_layout(
    _state: State<'_, Arc<AppState>>,
) -> Result<Option<String>, CommandError> {
    let path = crate::config::app_config::base_config_dir()
        .join("sessions")
        .join("orbital-layout.json");
    if path.exists() {
        std::fs::read_to_string(&path)
            .map(Some)
            .map_err(|e| CommandError::internal(e.to_string()))
    } else {
        Ok(None)
    }
}
