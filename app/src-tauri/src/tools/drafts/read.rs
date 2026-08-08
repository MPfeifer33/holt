use serde_json::json;

use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolVerificationStatus,
};

pub struct DraftReadTool;

#[async_trait::async_trait]
impl Tool for DraftReadTool {
    fn name(&self) -> &'static str {
        "draft_read"
    }

    fn description(&self) -> &'static str {
        "Read the full content of a named draft."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"name": "research-notes"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Draft name to read"
                }
            },
            "required": ["name"]
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
                message: "Missing required parameter: name".into(),
                retryable: false,
            })?;

        let filename = super::sanitize_name(name);
        let path = super::drafts_dir(&context.agent_id).join(&filename);

        let content = std::fs::read_to_string(&path).map_err(|e| ToolError {
            code: ToolErrorCode::InvalidInput,
            message: format!("Draft '{}' not found: {}", filename, e),
            retryable: false,
        })?;

        Ok(ToolResult {
            content: json!({
                "tool_result_status": "success",
                "name": filename,
                "content": content,
                "size_bytes": content.len(),
                "receipt": ToolExecutionReceipt {
                    authority_class: ToolAuthorityClass::Informational,
                    executed: true,
                    execution_status: "success".to_string(),
                    verified: false,
                    verification_status: ToolVerificationStatus::NotRequired,
                    execution_id: None,
                    tool_name: None,
                    tool_call_id: None,
                    tool_call_trace_id: None,
                    tool_result_trace_id: None,
                    summary: Some(format!("Read draft {}", filename)),
                }
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
