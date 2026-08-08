use serde_json::json;
use std::io::Write;

use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolVerificationStatus,
};

pub struct DraftWriteTool;

#[async_trait::async_trait]
impl Tool for DraftWriteTool {
    fn name(&self) -> &'static str {
        "draft_write"
    }

    fn description(&self) -> &'static str {
        "Save work-in-progress content to a named draft that persists across sessions. Use append=true to accumulate content over multiple turns."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"name": "research-notes", "content": "Key findings from today..."}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Draft name (used as filename). e.g., 'task-board-research'"
                },
                "content": {
                    "type": "string",
                    "description": "The draft content to write"
                },
                "append": {
                    "type": "boolean",
                    "description": "If true, append to existing draft instead of overwriting. Default: false"
                }
            },
            "required": ["name", "content"]
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

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: content".into(),
                retryable: false,
            })?;

        let append = arguments
            .get("append")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let dir = super::drafts_dir(&context.agent_id);
        std::fs::create_dir_all(&dir).map_err(|e| ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("Failed to create drafts directory: {e}"),
            retryable: false,
        })?;

        let filename = super::sanitize_name(name);
        let path = dir.join(&filename);

        if append {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| ToolError {
                    code: ToolErrorCode::InternalError,
                    message: format!("Failed to open draft for append: {e}"),
                    retryable: false,
                })?;
            file.write_all(content.as_bytes()).map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to append to draft: {e}"),
                retryable: false,
            })?;
        } else {
            std::fs::write(&path, content).map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to write draft: {e}"),
                retryable: false,
            })?;
        }

        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        Ok(ToolResult {
            content: json!({
                "tool_result_status": "success",
                "success": true,
                "name": filename,
                "size_bytes": size,
                "appended": append,
                "message": if append { "Content appended to draft" } else { "Draft saved" },
                "receipt": ToolExecutionReceipt {
                    authority_class: ToolAuthorityClass::Effectful,
                    executed: true,
                    execution_status: "success".to_string(),
                    verified: false,
                    verification_status: ToolVerificationStatus::NotRequired,
                    execution_id: None,
                    tool_name: None,
                    tool_call_id: None,
                    tool_call_trace_id: None,
                    tool_result_trace_id: None,
                    summary: Some(if append {
                        format!("Appended content to draft {}", filename)
                    } else {
                        format!("Saved draft {}", filename)
                    }),
                }
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
