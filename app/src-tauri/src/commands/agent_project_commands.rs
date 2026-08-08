//! Tauri commands wrapping `config::agent_projects` for the TUI project
//! switcher. Thin shims — all logic lives in `agent_projects`.

use crate::config::agent_projects::{self, ProjectEntry};
use std::path::PathBuf;

#[tauri::command]
pub fn list_projects(agent_id: String) -> Result<Vec<ProjectEntry>, String> {
    agent_projects::list(&agent_id)
}

#[tauri::command]
pub fn add_project(agent_id: String, name: String, path: String) -> Result<(), String> {
    agent_projects::add(&agent_id, &name, &PathBuf::from(path))
}

#[tauri::command]
pub fn remove_project(agent_id: String, name: String) -> Result<(), String> {
    agent_projects::remove(&agent_id, &name)
}
