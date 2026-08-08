use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

use crate::runtime::agent::AgentStatus;
use crate::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JobType {
    Message,
    FileWatch,
}

fn default_job_type() -> JobType {
    JobType::Message
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WatchEventType {
    Create,
    Modify,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchConfig {
    pub path: std::path::PathBuf,
    pub recursive: bool,
    pub glob_filter: Option<String>,
    pub events: Vec<WatchEventType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJob {
    pub id: String,
    pub name: String,
    pub agent_id: String,
    #[serde(default)]
    pub cron_expression: Option<String>,
    pub message: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_fired: Option<DateTime<Utc>>,
    pub last_skipped: Option<DateTime<Utc>>,
    pub fire_count: u64,
    pub skip_count: u64,
    #[serde(default = "default_job_type")]
    pub job_type: JobType,
    #[serde(default)]
    pub internal: bool,
    #[serde(default)]
    pub watch_config: Option<WatchConfig>,
}

pub struct TriggerRegistry {
    jobs: RwLock<Vec<ScheduledJob>>,
    config_dir: PathBuf,
}

impl TriggerRegistry {
    /// Load the registry from `trigger_jobs.json` in `config_dir`, falling back to empty.
    pub fn load(config_dir: PathBuf) -> Self {
        let new_path = config_dir.join("trigger_jobs.json");
        let old_path = config_dir.join("cron_jobs.json");

        let (jobs, path) = if new_path.exists() {
            let jobs = match std::fs::read_to_string(&new_path) {
                Ok(data) => match serde_json::from_str::<Vec<ScheduledJob>>(&data) {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        tracing::error!("trigger_jobs.json is corrupt, backing up: {e}");
                        let backup = config_dir.join("trigger_jobs.json.bak");
                        let _ = std::fs::copy(&new_path, &backup);
                        Vec::new()
                    }
                },
                Err(e) => {
                    tracing::warn!("Failed to read trigger_jobs.json: {e}");
                    Vec::new()
                }
            };
            (jobs, new_path)
        } else if old_path.exists() {
            // One-time migration from cron_jobs.json
            tracing::info!("Migrating cron_jobs.json → trigger_jobs.json");
            let jobs = match std::fs::read_to_string(&old_path) {
                Ok(data) => {
                    let parsed: Vec<ScheduledJob> = serde_json::from_str(&data).unwrap_or_default();
                    if let Ok(json) = serde_json::to_string_pretty(&parsed) {
                        let _ = std::fs::write(&new_path, json);
                    }
                    parsed
                }
                Err(_) => Vec::new(),
            };
            (jobs, new_path)
        } else {
            (Vec::new(), new_path)
        };
        tracing::info!("Loaded {} trigger job(s) from {:?}", jobs.len(), path);
        Self {
            jobs: RwLock::new(jobs),
            config_dir,
        }
    }

    /// Atomic write (tmp + rename) to trigger_jobs.json.
    async fn save(&self) {
        let jobs = self.jobs.read().await;
        let path = self.config_dir.join("trigger_jobs.json");
        let tmp_path = self.config_dir.join("trigger_jobs.json.tmp");

        match serde_json::to_string_pretty(&*jobs) {
            Ok(data) => {
                if let Err(e) = std::fs::write(&tmp_path, &data) {
                    tracing::warn!("Failed to write trigger_jobs tmp file: {e}");
                    return;
                }
                if let Err(e) = std::fs::rename(&tmp_path, &path) {
                    tracing::warn!("Failed to rename trigger_jobs tmp file: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("Failed to serialize trigger jobs: {e}");
            }
        }
    }

    /// Return a cloned list of all trigger jobs.
    pub async fn list(&self) -> Vec<ScheduledJob> {
        self.jobs.read().await.clone()
    }

    /// Return only user-created (non-internal) trigger jobs.
    pub async fn list_visible(&self) -> Vec<ScheduledJob> {
        self.jobs
            .read()
            .await
            .iter()
            .filter(|j| !j.internal)
            .cloned()
            .collect()
    }

    /// Get a single cron job by ID.
    pub async fn get(&self, id: &str) -> Option<ScheduledJob> {
        self.jobs.read().await.iter().find(|j| j.id == id).cloned()
    }

    /// Create a new scheduled trigger. Validates the cron expression before saving.
    pub async fn create(
        &self,
        name: String,
        agent_id: String,
        cron_expression: String,
        message: String,
    ) -> Result<ScheduledJob, String> {
        validate_cron(&cron_expression)?;

        let job = ScheduledJob {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            agent_id,
            cron_expression: Some(cron_expression),
            message,
            enabled: true,
            created_at: Utc::now(),
            last_fired: None,
            last_skipped: None,
            fire_count: 0,
            skip_count: 0,
            job_type: JobType::Message,
            internal: false,
            watch_config: None,
        };

        {
            let mut jobs = self.jobs.write().await;
            jobs.push(job.clone());
        }
        self.save().await;
        Ok(job)
    }

    /// Create an internal trigger (e.g., dream jobs, default watches). Not visible in user-facing lists.
    pub async fn create_internal(
        &self,
        name: String,
        agent_id: String,
        cron_expression: String,
        job_type: JobType,
    ) -> Result<ScheduledJob, String> {
        validate_cron(&cron_expression)?;

        let job = ScheduledJob {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            agent_id,
            cron_expression: Some(cron_expression),
            message: String::new(),
            enabled: true,
            created_at: Utc::now(),
            last_fired: None,
            last_skipped: None,
            fire_count: 0,
            skip_count: 0,
            job_type,
            internal: true,
            watch_config: None,
        };

        {
            let mut jobs = self.jobs.write().await;
            jobs.push(job.clone());
        }
        self.save().await;
        Ok(job)
    }

    /// Partial update of a cron job's fields.
    pub async fn update(
        &self,
        id: &str,
        name: Option<String>,
        cron_expression: Option<String>,
        message: Option<String>,
        enabled: Option<bool>,
    ) -> Result<ScheduledJob, String> {
        if let Some(ref expr) = cron_expression {
            validate_cron(expr)?;
        }

        let mut jobs = self.jobs.write().await;
        let job = jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| format!("Trigger job not found: {id}"))?;

        if let Some(name) = name {
            job.name = name;
        }
        if let Some(expr) = cron_expression {
            job.cron_expression = Some(expr);
        }
        if let Some(message) = message {
            job.message = message;
        }
        if let Some(enabled) = enabled {
            job.enabled = enabled;
        }

        let updated = job.clone();
        drop(jobs);
        self.save().await;
        Ok(updated)
    }

    /// Delete a cron job by ID.
    pub async fn delete(&self, id: &str) -> Result<(), String> {
        let mut jobs = self.jobs.write().await;
        let idx = jobs
            .iter()
            .position(|j| j.id == id)
            .ok_or_else(|| format!("Trigger job not found: {id}"))?;
        jobs.remove(idx);
        drop(jobs);
        self.save().await;
        Ok(())
    }

    /// Toggle a cron job's enabled state.
    pub async fn toggle(&self, id: &str) -> Result<ScheduledJob, String> {
        let mut jobs = self.jobs.write().await;
        let job = jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| format!("Trigger job not found: {id}"))?;

        job.enabled = !job.enabled;

        let toggled = job.clone();
        drop(jobs);
        self.save().await;
        Ok(toggled)
    }

    /// Remove all trigger jobs for a given agent. Returns the removed jobs.
    pub async fn remove_jobs_for_agent(&self, agent_id: &str) -> Vec<ScheduledJob> {
        let mut jobs = self.jobs.write().await;
        let mut removed = Vec::new();
        jobs.retain(|j| {
            if j.agent_id == agent_id {
                removed.push(j.clone());
                false
            } else {
                true
            }
        });
        drop(jobs);
        if !removed.is_empty() {
            self.save().await;
        }
        removed
    }

    /// Create a file watch trigger.
    pub async fn create_file_watch(
        &self,
        name: String,
        agent_id: String,
        watch_config: WatchConfig,
        internal: bool,
    ) -> Result<ScheduledJob, String> {
        let job = ScheduledJob {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            agent_id,
            cron_expression: None,
            message: String::new(),
            enabled: true,
            created_at: Utc::now(),
            last_fired: None,
            last_skipped: None,
            fire_count: 0,
            skip_count: 0,
            job_type: JobType::FileWatch,
            internal,
            watch_config: Some(watch_config),
        };

        {
            let mut jobs = self.jobs.write().await;
            jobs.push(job.clone());
        }
        self.save().await;
        Ok(job)
    }

    /// Get all file watch jobs for an agent.
    pub async fn file_watch_jobs_for_agent(&self, agent_id: &str) -> Vec<ScheduledJob> {
        let jobs = self.jobs.read().await;
        jobs.iter()
            .filter(|j| j.agent_id == agent_id && j.job_type == JobType::FileWatch)
            .cloned()
            .collect()
    }

    /// Count custom (non-internal) file watches for an agent.
    pub async fn custom_watch_count(&self, agent_id: &str) -> usize {
        let jobs = self.jobs.read().await;
        jobs.iter()
            .filter(|j| j.agent_id == agent_id && j.job_type == JobType::FileWatch && !j.internal)
            .count()
    }

    /// Record that a job fired (update stats).
    pub async fn record_fire(&self, job_id: &str) {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.iter_mut().find(|j| j.id == job_id) {
            job.last_fired = Some(Utc::now());
            job.fire_count += 1;
        }
        drop(jobs);
        self.save().await;
    }

    /// Record that a job was skipped (update stats).
    pub async fn record_skip(&self, job_id: &str) {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.iter_mut().find(|j| j.id == job_id) {
            job.last_skipped = Some(Utc::now());
            job.skip_count += 1;
        }
        drop(jobs);
        self.save().await;
    }

    /// Spawn a background task that ticks every 60 seconds checking for due triggers.
    pub fn spawn_scheduler(self: &Arc<Self>, state: Arc<AppState>, app_handle: AppHandle) {
        let registry = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.tick().await; // Skip immediate first tick
            loop {
                interval.tick().await;
                registry.tick(&state, &app_handle).await;
            }
        });
    }

    /// Core scheduler tick: check all enabled jobs against current time and fire matches.
    async fn tick(&self, state: &Arc<AppState>, app_handle: &AppHandle) {
        // Master kill switch
        let enabled = state.config.read().await.cron.enabled;
        if !enabled {
            return;
        }

        let now = Utc::now();
        let jobs = self.jobs.read().await.clone();
        let mut any_stats_changed = false;

        for job in &jobs {
            if !job.enabled {
                continue;
            }
            // Skip file watch jobs — they're handled by FileWatchManager
            if job.job_type == JobType::FileWatch {
                continue;
            }
            let cron_expr = match &job.cron_expression {
                Some(expr) => expr,
                None => continue, // No cron expression = not time-based
            };
            if !cron_matches(cron_expr, &now) {
                continue;
            }

            // Check agent status
            let agent_status = {
                let agents = state.agents.read().await;
                agents
                    .iter()
                    .find(|a| a.id == job.agent_id)
                    .map(|a| a.status.clone())
            };

            match agent_status {
                Some(AgentStatus::Idle) => {
                    // Fire the job
                    {
                        let mut jobs_w = self.jobs.write().await;
                        if let Some(j) = jobs_w.iter_mut().find(|j| j.id == job.id) {
                            j.last_fired = Some(now);
                            j.fire_count += 1;
                        }
                    }
                    any_stats_changed = true;

                    let state_clone = state.clone();
                    let app_clone = app_handle.clone();
                    let agent_id = job.agent_id.clone();

                    match job.job_type {
                        JobType::Message => {
                            let message = format!("[Scheduled: {}] {}", job.name, job.message);
                            let job_name = job.name.clone();
                            tauri::async_runtime::spawn(async move {
                                // Ensure TurnManager worker exists
                                crate::commands::agent_commands::messaging::register_turn_worker(
                                    &state_clone,
                                    &app_clone,
                                    &agent_id,
                                )
                                .await;

                                let protocol = {
                                    let agents = state_clone.agents.read().await;
                                    agents
                                        .iter()
                                        .find(|a| a.id == agent_id)
                                        .map(|a| a.protocol.clone())
                                        .unwrap_or_default()
                                };
                                let turn_protocol =
                                    crate::runtime::turn_manager::TurnProtocol::from_message(
                                        &protocol, message, None, None,
                                    );
                                let request = crate::runtime::turn_manager::TurnRequest::new(
                                    turn_protocol,
                                    crate::runtime::turn_manager::TurnSource::ScheduledTrigger {
                                        job_name,
                                    },
                                    crate::runtime::turn_manager::TurnPriority::Background,
                                );
                                if let Some(handle) = state_clone
                                    .turn_manager
                                    .submit_turn(&agent_id, request)
                                    .await
                                {
                                    if let crate::runtime::turn_manager::TurnResult::Error(e) =
                                        handle.result().await
                                    {
                                        tracing::warn!(
                                            "Trigger fire failed for agent {}: {}",
                                            agent_id,
                                            e
                                        );
                                    }
                                }
                            });
                            tracing::info!("Trigger fired: {} → agent {}", job.name, job.agent_id);
                        }
                        JobType::FileWatch => {
                            // FileWatch handled by FileWatchManager
                            unreachable!("FileWatch jobs should be handled before this point");
                        }
                    }

                    let _ = app_handle.emit(
                        "trigger-fired",
                        json!({
                            "job_id": job.id,
                            "agent_id": job.agent_id,
                            "job_name": job.name,
                        }),
                    );
                }
                Some(_) => {
                    // Agent is busy — skip
                    {
                        let mut jobs_w = self.jobs.write().await;
                        if let Some(j) = jobs_w.iter_mut().find(|j| j.id == job.id) {
                            j.last_skipped = Some(now);
                            j.skip_count += 1;
                        }
                    }
                    any_stats_changed = true;

                    let _ = app_handle.emit(
                        "trigger-skipped",
                        json!({
                            "job_id": job.id,
                            "agent_id": job.agent_id,
                            "job_name": job.name,
                            "reason": "agent_busy",
                        }),
                    );

                    tracing::debug!(
                        "Trigger skipped (agent busy): {} → agent {}",
                        job.name,
                        job.agent_id
                    );
                }
                None => {
                    tracing::warn!(
                        "Trigger job {} references missing agent {}",
                        job.id,
                        job.agent_id
                    );
                }
            }
        }

        if any_stats_changed {
            self.save().await;
        }

        // Periodic orphan sweep for subagent pending results
        let active_ids: Vec<String> = {
            let agents = state.agents.read().await;
            agents.iter().map(|a| a.id.clone()).collect()
        };
        state
            .subagent_manager
            .sweep_orphaned_results(&active_ids)
            .await;
    }

    /// Immediately fire a trigger regardless of schedule. FileWatch jobs cannot be manually fired.
    pub async fn fire_now(
        &self,
        id: &str,
        state: &Arc<AppState>,
        app_handle: &AppHandle,
    ) -> Result<(), String> {
        let job = self
            .get(id)
            .await
            .ok_or_else(|| format!("Job not found: {}", id))?;
        if job.job_type == JobType::FileWatch {
            return Err(
                "File watch triggers cannot be manually fired -- they respond to file changes"
                    .into(),
            );
        }

        let message = format!("[Scheduled: {}] {}", job.name, job.message);
        {
            let mut jobs = self.jobs.write().await;
            if let Some(j) = jobs.iter_mut().find(|j| j.id == id) {
                j.last_fired = Some(Utc::now());
                j.fire_count += 1;
            }
        }
        self.save().await;

        // Ensure TurnManager worker exists
        crate::commands::agent_commands::messaging::register_turn_worker(
            state,
            app_handle,
            &job.agent_id,
        )
        .await;

        let protocol = {
            let agents = state.agents.read().await;
            agents
                .iter()
                .find(|a| a.id == job.agent_id)
                .map(|a| a.protocol.clone())
                .unwrap_or_default()
        };
        let turn_protocol = crate::runtime::turn_manager::TurnProtocol::from_message(
            &protocol, message, None, None,
        );
        let request = crate::runtime::turn_manager::TurnRequest::new(
            turn_protocol,
            crate::runtime::turn_manager::TurnSource::ScheduledTrigger {
                job_name: job.name.clone(),
            },
            crate::runtime::turn_manager::TurnPriority::Background,
        );
        match state.turn_manager.submit_turn(&job.agent_id, request).await {
            Some(handle) => match handle.result().await {
                crate::runtime::turn_manager::TurnResult::Completed(_) => Ok(()),
                crate::runtime::turn_manager::TurnResult::Error(e) => Err(e),
                crate::runtime::turn_manager::TurnResult::Cancelled => {
                    Err("Turn was cancelled".into())
                }
                crate::runtime::turn_manager::TurnResult::Dropped => Err("Turn was dropped".into()),
            },
            None => Err("Turn queue full or agent not registered".into()),
        }
    }
}

/// Validate a cron expression string.
pub fn validate_cron(expr: &str) -> Result<(), String> {
    croner::Cron::from_str(expr).map_err(|e| format!("Invalid cron expression: {}", e))?;
    Ok(())
}

/// Check if a cron expression matches the given time.
pub fn cron_matches(expr: &str, now: &DateTime<Utc>) -> bool {
    let Ok(cron) = croner::Cron::from_str(expr) else {
        return false;
    };
    cron.is_time_matching(now).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_validate_cron_valid() {
        assert!(validate_cron("*/5 * * * *").is_ok());
        assert!(validate_cron("0 9 * * 1").is_ok());
        assert!(validate_cron("0 2 * * *").is_ok());
    }

    #[test]
    fn test_validate_cron_invalid() {
        assert!(validate_cron("not a cron").is_err());
        assert!(validate_cron("").is_err());
    }

    #[test]
    fn test_cron_matches_known_time() {
        // 2026-03-24 09:00:00 UTC is a Tuesday
        let time = Utc.with_ymd_and_hms(2026, 3, 24, 9, 0, 0).unwrap();
        assert!(cron_matches("0 9 * * *", &time)); // every day at 9am
        assert!(cron_matches("* * * * *", &time)); // every minute
        assert!(!cron_matches("0 10 * * *", &time)); // 10am, not 9am
    }

    #[test]
    fn test_cron_job_serde_roundtrip() {
        let job = ScheduledJob {
            id: "test-123".to_string(),
            name: "Test Job".to_string(),
            agent_id: "agent-1".to_string(),
            cron_expression: Some("*/5 * * * *".to_string()),
            message: "do something".to_string(),
            enabled: true,
            created_at: Utc::now(),
            last_fired: None,
            last_skipped: None,
            fire_count: 0,
            skip_count: 0,
            job_type: JobType::Message,
            internal: false,
            watch_config: None,
        };
        let json = serde_json::to_string(&job).unwrap();
        let decoded: ScheduledJob = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, "test-123");
        assert!(decoded.enabled);
        assert_eq!(decoded.job_type, JobType::Message);
        assert!(!decoded.internal);
    }

    #[tokio::test]
    async fn test_crud_operations() {
        let dir = tempfile::tempdir().unwrap();
        let registry = TriggerRegistry::load(dir.path().to_path_buf());

        let job = registry
            .create(
                "Test".into(),
                "agent-1".into(),
                "*/5 * * * *".into(),
                "hello".into(),
            )
            .await
            .unwrap();
        assert_eq!(job.name, "Test");

        assert_eq!(registry.list().await.len(), 1);
        assert_eq!(registry.get(&job.id).await.unwrap().name, "Test");

        let updated = registry
            .update(&job.id, Some("Updated".into()), None, None, None)
            .await
            .unwrap();
        assert_eq!(updated.name, "Updated");

        let toggled = registry.toggle(&job.id).await.unwrap();
        assert!(!toggled.enabled);

        registry.delete(&job.id).await.unwrap();
        assert!(registry.list().await.is_empty());
    }

    #[tokio::test]
    async fn test_remove_jobs_for_agent() {
        let dir = tempfile::tempdir().unwrap();
        let registry = TriggerRegistry::load(dir.path().to_path_buf());

        registry
            .create("A".into(), "agent-1".into(), "* * * * *".into(), "m".into())
            .await
            .unwrap();
        registry
            .create("B".into(), "agent-1".into(), "* * * * *".into(), "m".into())
            .await
            .unwrap();
        registry
            .create("C".into(), "agent-2".into(), "* * * * *".into(), "m".into())
            .await
            .unwrap();

        let removed = registry.remove_jobs_for_agent("agent-1").await;
        assert_eq!(removed.len(), 2);
        assert_eq!(registry.list().await.len(), 1);
    }

    #[tokio::test]
    async fn test_create_invalid_cron_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let registry = TriggerRegistry::load(dir.path().to_path_buf());
        assert!(registry
            .create("Bad".into(), "a".into(), "not valid".into(), "m".into())
            .await
            .is_err());
    }

    #[test]
    fn test_job_type_serde_default() {
        // Existing jobs without job_type should deserialize as Message
        let json = r#"{"id":"1","name":"t","agent_id":"a","cron_expression":"* * * * *","message":"m","enabled":true,"created_at":"2026-01-01T00:00:00Z","last_fired":null,"last_skipped":null,"fire_count":0,"skip_count":0}"#;
        let job: ScheduledJob = serde_json::from_str(json).unwrap();
        assert_eq!(job.job_type, JobType::Message);
        assert!(!job.internal);
    }

    #[test]
    fn test_internal_job_type_serde() {
        let job = ScheduledJob {
            id: "dl-1".to_string(),
            name: "internal-job".to_string(),
            agent_id: "agent-1".to_string(),
            cron_expression: Some("0 10 * * *".to_string()),
            message: String::new(),
            enabled: true,
            created_at: Utc::now(),
            last_fired: None,
            last_skipped: None,
            fire_count: 0,
            skip_count: 0,
            job_type: JobType::FileWatch,
            internal: true,
            watch_config: None,
        };
        let json = serde_json::to_string(&job).unwrap();
        let decoded: ScheduledJob = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.job_type, JobType::FileWatch);
        assert!(decoded.internal);
    }

    #[tokio::test]
    async fn test_create_internal_job() {
        let dir = tempfile::tempdir().unwrap();
        let registry = TriggerRegistry::load(dir.path().to_path_buf());

        let job = registry
            .create_internal(
                "internal-job".into(),
                "agent-1".into(),
                "0 10 * * *".into(),
                JobType::FileWatch,
            )
            .await
            .unwrap();

        assert_eq!(job.job_type, JobType::FileWatch);
        assert!(job.internal);
        assert_eq!(job.message, "");

        // Internal jobs should not appear in list_visible
        assert!(registry.list_visible().await.is_empty());
        // But should appear in list (all)
        assert_eq!(registry.list().await.len(), 1);
    }
}
