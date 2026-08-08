use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct CancelSubagentTool;

#[async_trait::async_trait]
impl Tool for CancelSubagentTool {
    fn name(&self) -> &'static str {
        "cancel_subagent"
    }

    fn description(&self) -> &'static str {
        "Cancel a running subagent and retrieve any partial result."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"job_id": "abc-123"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string", "description": "The job_id of the subagent to cancel" }
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

        let partial_result = context.subagent_manager.cancel(job_id).await;

        Ok(ToolResult {
            content: json!({
                "job_id": job_id,
                "status": "cancelled",
                "partial_result": partial_result,
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
