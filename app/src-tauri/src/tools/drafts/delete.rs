use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct DraftDeleteTool;

#[async_trait::async_trait]
impl Tool for DraftDeleteTool {
    fn name(&self) -> &'static str {
        "draft_delete"
    }

    fn description(&self) -> &'static str {
        "Delete a draft you no longer need."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"name": "old-draft"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Draft name to delete"
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

        if !path.exists() {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!("Draft '{}' not found", filename),
                retryable: false,
            });
        }

        std::fs::remove_file(&path).map_err(|e| ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("Failed to delete draft: {e}"),
            retryable: false,
        })?;

        Ok(ToolResult {
            content: json!({
                "success": true,
                "name": filename,
                "message": "Draft deleted"
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
