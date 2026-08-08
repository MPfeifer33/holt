use async_trait::async_trait;
use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

/// Maximum scheduled triggers per agent, matching the watch limit.
const MAX_AGENT_SCHEDULES: usize = 10;

// ============================================
// cron_create
// ============================================

pub struct CronCreateTool;

#[async_trait]
impl Tool for CronCreateTool {
    fn name(&self) -> &str {
        "cron_create"
    }

    fn description(&self) -> &str {
        "Schedule a recurring message to yourself on a cron schedule (e.g., every 2 hours). The message arrives when you're idle. Max 10 schedules."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(
            json!({"name": "health-check", "cron_expression": "0 */2 * * *", "message": "Run a health check"}),
        )
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Friendly name for this schedule (e.g., 'hourly-health-check')"
                },
                "cron_expression": {
                    "type": "string",
                    "description": "Cron expression (e.g., '0 */2 * * *' for every 2 hours, '30 9 * * 1-5' for 9:30 AM weekdays)"
                },
                "message": {
                    "type": "string",
                    "description": "Message that will be sent to you when the schedule fires"
                }
            },
            "required": ["name", "cron_expression", "message"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "name is required".into(),
                retryable: false,
            })?;

        let cron_expression = arguments
            .get("cron_expression")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "cron_expression is required".into(),
                retryable: false,
            })?;

        let message = arguments
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "message is required".into(),
                retryable: false,
            })?;

        let app_state = context.app_state.as_ref().ok_or_else(|| ToolError {
            code: ToolErrorCode::PermissionDenied,
            message: "AppState not available".into(),
            retryable: false,
        })?;

        // Check per-agent limit (only count non-internal Message-type jobs owned by this agent)
        let all_jobs = app_state.trigger_registry.list().await;
        let agent_schedule_count = all_jobs
            .iter()
            .filter(|j| {
                j.agent_id == context.agent_id
                    && !j.internal
                    && matches!(j.job_type, crate::runtime::triggers::JobType::Message)
            })
            .count();

        if agent_schedule_count >= MAX_AGENT_SCHEDULES {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!(
                    "Maximum {} scheduled triggers reached. Use cron_delete to remove one first.",
                    MAX_AGENT_SCHEDULES
                ),
                retryable: false,
            });
        }

        let job = app_state
            .trigger_registry
            .create(
                name.to_string(),
                context.agent_id.clone(),
                cron_expression.to_string(),
                message.to_string(),
            )
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: e,
                retryable: false,
            })?;

        Ok(ToolResult {
            content: json!({
                "success": true,
                "job_id": job.id,
                "name": job.name,
                "cron_expression": cron_expression,
                "message": format!("Schedule '{}' created. It will fire when you are idle and the cron expression matches.", name),
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

// ============================================
// cron_list
// ============================================

pub struct CronListTool;

#[async_trait]
impl Tool for CronListTool {
    fn name(&self) -> &str {
        "cron_list"
    }

    fn description(&self) -> &str {
        "List your scheduled cron triggers with name, schedule, and fire statistics."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(
        &self,
        _arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let app_state = context.app_state.as_ref().ok_or_else(|| ToolError {
            code: ToolErrorCode::PermissionDenied,
            message: "AppState not available".into(),
            retryable: false,
        })?;

        let all_jobs = app_state.trigger_registry.list().await;
        let my_jobs: Vec<_> = all_jobs
            .into_iter()
            .filter(|j| {
                j.agent_id == context.agent_id
                    && matches!(j.job_type, crate::runtime::triggers::JobType::Message)
            })
            .collect();

        let entries: Vec<serde_json::Value> = my_jobs
            .iter()
            .map(|j| {
                json!({
                    "job_id": j.id,
                    "name": j.name,
                    "cron_expression": j.cron_expression,
                    "message": j.message,
                    "enabled": j.enabled,
                    "internal": j.internal,
                    "fire_count": j.fire_count,
                    "skip_count": j.skip_count,
                    "last_fired": j.last_fired.map(|t| t.to_rfc3339()),
                    "created_at": j.created_at.to_rfc3339(),
                })
            })
            .collect();

        Ok(ToolResult {
            content: json!({
                "schedules": entries,
                "count": entries.len(),
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

// ============================================
// cron_delete
// ============================================

pub struct CronDeleteTool;

#[async_trait]
impl Tool for CronDeleteTool {
    fn name(&self) -> &str {
        "cron_delete"
    }

    fn description(&self) -> &str {
        "Delete a scheduled cron trigger by job ID."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"job_id": "abc-123"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": {
                    "type": "string",
                    "description": "The job_id of the schedule to delete (from cron_list)"
                }
            },
            "required": ["job_id"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let job_id = arguments
            .get("job_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "job_id is required".into(),
                retryable: false,
            })?;

        let app_state = context.app_state.as_ref().ok_or_else(|| ToolError {
            code: ToolErrorCode::PermissionDenied,
            message: "AppState not available".into(),
            retryable: false,
        })?;

        // Verify ownership — agent can only delete its own jobs
        let job = app_state
            .trigger_registry
            .get(job_id)
            .await
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!("Schedule not found: {}", job_id),
                retryable: false,
            })?;

        if job.agent_id != context.agent_id {
            return Err(ToolError {
                code: ToolErrorCode::PermissionDenied,
                message: "You can only delete your own schedules".into(),
                retryable: false,
            });
        }

        // Internal jobs (like dream schedules) can be toggled off but not deleted by the agent
        if job.internal {
            return Err(ToolError {
                code: ToolErrorCode::PermissionDenied,
                message:
                    "Internal schedules cannot be deleted. Use the Triggers panel to manage them."
                        .into(),
                retryable: false,
            });
        }

        app_state
            .trigger_registry
            .delete(job_id)
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: e,
                retryable: false,
            })?;

        Ok(ToolResult {
            content: json!({
                "success": true,
                "job_id": job_id,
                "name": job.name,
                "message": format!("Schedule '{}' deleted", job.name),
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
