use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, State};

use super::error::CommandError;
use crate::runtime::triggers::ScheduledJob;
use crate::AppState;

#[derive(Serialize)]
pub struct WatchConfigResponse {
    pub path: String,
    pub recursive: bool,
    pub glob_filter: Option<String>,
    pub events: Vec<String>,
}

#[derive(Serialize)]
pub struct TriggerJobResponse {
    pub id: String,
    pub name: String,
    pub agent_id: String,
    pub cron_expression: Option<String>,
    pub message: String,
    pub enabled: bool,
    pub created_at: String,
    pub last_fired: Option<String>,
    pub last_skipped: Option<String>,
    pub fire_count: u64,
    pub skip_count: u64,
    pub next_fire: Option<String>,
    pub job_type: String,
    pub watch_config: Option<WatchConfigResponse>,
    pub internal: bool,
}

impl From<ScheduledJob> for TriggerJobResponse {
    fn from(job: ScheduledJob) -> Self {
        let next_fire = job.cron_expression.as_ref().and_then(|expr| {
            use std::str::FromStr;
            croner::Cron::from_str(expr)
                .ok()
                .and_then(|c| c.find_next_occurrence(&chrono::Utc::now(), false).ok())
                .map(|dt| dt.to_rfc3339())
        });

        let job_type = match job.job_type {
            crate::runtime::triggers::JobType::Message => "message",
            crate::runtime::triggers::JobType::FileWatch => "file_watch",
        }
        .to_string();

        let watch_config = job.watch_config.as_ref().map(|c| WatchConfigResponse {
            path: c.path.display().to_string(),
            recursive: c.recursive,
            glob_filter: c.glob_filter.clone(),
            events: c
                .events
                .iter()
                .map(|e| format!("{:?}", e).to_lowercase())
                .collect(),
        });

        Self {
            id: job.id,
            name: job.name,
            agent_id: job.agent_id,
            cron_expression: job.cron_expression,
            message: job.message,
            enabled: job.enabled,
            created_at: job.created_at.to_rfc3339(),
            last_fired: job.last_fired.map(|dt| dt.to_rfc3339()),
            last_skipped: job.last_skipped.map(|dt| dt.to_rfc3339()),
            fire_count: job.fire_count,
            skip_count: job.skip_count,
            next_fire,
            job_type,
            watch_config,
            internal: job.internal,
        }
    }
}

#[tauri::command]
pub async fn create_trigger(
    state: State<'_, Arc<AppState>>,
    name: String,
    agent_id: String,
    cron_expression: String,
    message: String,
) -> Result<TriggerJobResponse, CommandError> {
    let job = state
        .trigger_registry
        .create(name, agent_id, cron_expression, message)
        .await
        .map_err(CommandError::validation)?;
    Ok(job.into())
}

#[tauri::command]
pub async fn list_triggers(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<TriggerJobResponse>, CommandError> {
    let jobs = state.trigger_registry.list_visible().await;
    Ok(jobs.into_iter().map(|j| j.into()).collect())
}

#[tauri::command]
pub async fn get_trigger(
    state: State<'_, Arc<AppState>>,
    job_id: String,
) -> Result<TriggerJobResponse, CommandError> {
    state
        .trigger_registry
        .get(&job_id)
        .await
        .map(|j| j.into())
        .ok_or_else(|| CommandError::not_found(format!("Cron job not found: {job_id}")))
}

#[tauri::command]
pub async fn update_trigger(
    state: State<'_, Arc<AppState>>,
    job_id: String,
    name: Option<String>,
    cron_expression: Option<String>,
    message: Option<String>,
    enabled: Option<bool>,
) -> Result<TriggerJobResponse, CommandError> {
    let job = state
        .trigger_registry
        .update(&job_id, name, cron_expression, message, enabled)
        .await
        .map_err(CommandError::not_found)?;
    Ok(job.into())
}

#[tauri::command]
pub async fn delete_trigger(
    state: State<'_, Arc<AppState>>,
    job_id: String,
) -> Result<(), CommandError> {
    state
        .trigger_registry
        .delete(&job_id)
        .await
        .map_err(CommandError::not_found)
}

#[tauri::command]
pub async fn toggle_trigger(
    state: State<'_, Arc<AppState>>,
    job_id: String,
) -> Result<TriggerJobResponse, CommandError> {
    let job = state
        .trigger_registry
        .toggle(&job_id)
        .await
        .map_err(CommandError::not_found)?;
    Ok(job.into())
}

#[tauri::command]
pub async fn fire_trigger(
    state: State<'_, Arc<AppState>>,
    app_handle: AppHandle,
    job_id: String,
) -> Result<(), CommandError> {
    let state_arc: Arc<AppState> = state.inner().clone();
    state_arc
        .trigger_registry
        .fire_now(&job_id, &state_arc, &app_handle)
        .await
        .map_err(CommandError::internal)
}
