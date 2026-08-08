use std::sync::Arc;

use tauri::State;

use super::error::CommandError;
use crate::config::app_config::base_config_dir;
use crate::trace::store::TraceRow;
use crate::trace::truncation::TraceOutcome;
use crate::AppState;

/// Serializable trace entry for frontend display.
#[derive(serde::Serialize)]
pub struct TraceEntry {
    pub id: String,
    pub timestamp: String,
    pub agent_id: String,
    pub trace_type: String,
    pub content: String,
    pub outcome: Option<String>,
}

/// Map a stored outcome JSON string to a clean snake_case display value.
/// Returns "success", "failure", "partial", or "unknown".
fn outcome_to_display(outcome_json: Option<String>) -> Option<String> {
    let s = outcome_json?;
    let tag = match serde_json::from_str::<TraceOutcome>(&s) {
        Ok(TraceOutcome::Success) => "success",
        Ok(TraceOutcome::Failure { .. }) => "failure",
        Ok(TraceOutcome::Partial { .. }) => "partial",
        Ok(TraceOutcome::Unknown) => "unknown",
        Err(_) => "unknown",
    };
    Some(tag.to_string())
}

/// Convert a raw TraceRow from the store into a serializable TraceEntry.
/// Content is passed through as-is (already truncated at write time) to avoid
/// JSON parse/reserialize round-tripping that re-escapes truncated markers.
fn row_to_entry(row: TraceRow) -> TraceEntry {
    TraceEntry {
        id: row.id,
        timestamp: row.timestamp,
        agent_id: row.agent_id,
        trace_type: row.trace_type,
        content: row.content,
        outcome: outcome_to_display(row.outcome),
    }
}

#[tauri::command]
pub async fn get_ui_config(
    state: State<'_, Arc<AppState>>,
) -> Result<crate::config::app_config::UiBlock, CommandError> {
    let config = state.config.read().await;
    Ok(config.ui.clone())
}

#[tauri::command]
pub async fn query_recent_traces(
    state: State<'_, Arc<AppState>>,
    limit: u32,
    agent_id: Option<String>,
) -> Result<Vec<TraceEntry>, CommandError> {
    let rows = match agent_id {
        Some(id) => state
            .trace_engine
            .query_by_agent_rows(&id, limit)
            .map_err(|e| CommandError::internal(e.to_string()))?,
        None => state
            .trace_engine
            .query_recent_rows(limit)
            .map_err(|e| CommandError::internal(e.to_string()))?,
    };

    Ok(rows.into_iter().map(row_to_entry).collect())
}

/// Export traces to JSON file, returns file path.
#[tauri::command]
pub async fn export_traces(state: State<'_, Arc<AppState>>) -> Result<String, CommandError> {
    let traces = state
        .trace_engine
        .query_recent_rows(1000)
        .map_err(|e| CommandError::internal(e.to_string()))?;
    let json =
        serde_json::to_string_pretty(&traces).map_err(|e| CommandError::internal(e.to_string()))?;

    let export_path = base_config_dir().join("traces_export.json");
    std::fs::write(&export_path, json).map_err(|e| CommandError::internal(e.to_string()))?;

    Ok(export_path.to_string_lossy().to_string())
}

/// Clear all traces.
#[tauri::command]
pub async fn clear_traces(state: State<'_, Arc<AppState>>) -> Result<(), CommandError> {
    state
        .trace_engine
        .clear_all()
        .map_err(|e| CommandError::internal(e.to_string()))
}
