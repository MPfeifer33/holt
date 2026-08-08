use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct CheckSubagentTool;

#[async_trait::async_trait]
impl Tool for CheckSubagentTool {
    fn name(&self) -> &'static str {
        "check_subagent"
    }

    fn description(&self) -> &'static str {
        "Check the status and result of a spawned subagent by job_id."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"job_id": "abc-123"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string", "description": "The job_id returned by spawn_subagent" }
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
                message: "Missing required parameter: job_id".to_string(),
                retryable: false,
            })?;

        let info = context
            .subagent_manager
            .check(job_id)
            .await
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!("No subagent found with job_id: {}", job_id),
                retryable: false,
            })?;

        Ok(ToolResult {
            content: json!({
                "job_id": info.job_id,
                "name": info.name,
                "task": info.task,
                "status": format!("{:?}", info.status),
                "elapsed_ms": info.elapsed_ms,
                "created_at": info.created_at,
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
