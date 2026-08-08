//! Tauri commands for terminal management.

use std::sync::Arc;
use tauri::{AppHandle, State};

use super::manager::TerminalManager;
use super::session::TerminalInfo;

/// Create a new terminal session.
#[tauri::command]
pub async fn terminal_create(
    state: State<'_, Arc<TerminalManager>>,
    app_handle: AppHandle,
    shell: Option<String>,
    cwd: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<TerminalInfo, String> {
    state
        .inner()
        .create(shell, cwd, cols, rows, app_handle)
        .await
}

/// Destroy a terminal session.
#[tauri::command]
pub async fn terminal_destroy(
    state: State<'_, Arc<TerminalManager>>,
    session_id: String,
) -> Result<(), String> {
    state.inner().destroy(&session_id).await
}

/// Write data to a terminal session (user input).
#[tauri::command]
pub async fn terminal_write(
    state: State<'_, Arc<TerminalManager>>,
    session_id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    state.inner().write(&session_id, &data).await
}

/// Resize a terminal session.
#[tauri::command]
pub async fn terminal_resize(
    state: State<'_, Arc<TerminalManager>>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.inner().resize(&session_id, cols, rows).await
}

/// List all active terminal sessions.
#[tauri::command]
pub async fn terminal_list(
    state: State<'_, Arc<TerminalManager>>,
) -> Result<Vec<TerminalInfo>, String> {
    Ok(state.inner().list().await)
}
