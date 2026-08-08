use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct ReadFullOutputTool;

#[async_trait::async_trait]
impl Tool for ReadFullOutputTool {
    fn name(&self) -> &'static str {
        "read_full_output"
    }

    fn description(&self) -> &'static str {
        "Retrieve the full, untruncated output of a previous tool call from traces."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"trace_id": "abc-123"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "trace_id": { "type": "string", "description": "Trace ID of the truncated tool call" },
                "offset": { "type": "integer", "description": "Start from this line (for paging)" },
                "limit": { "type": "integer", "description": "Max lines to return (default: 200)" }
            },
            "required": ["trace_id"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let trace_id = arguments
            .get("trace_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: trace_id".to_string(),
                retryable: false,
            })?;

        let offset = arguments
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(200) as usize;

        // Query trace DB for the content
        let trace = context
            .trace_engine
            .read_trace(trace_id)
            .map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to read trace: {}", e),
                retryable: true,
            })?;

        let content_str = match trace {
            Some(t) => t.content.to_string(),
            None => {
                return Err(ToolError {
                    code: ToolErrorCode::InvalidInput,
                    message: format!("Trace '{}' not found", trace_id),
                    retryable: false,
                });
            }
        };

        let all_lines: Vec<&str> = content_str.lines().collect();
        let total_lines = all_lines.len() as u32;

        let start = offset.min(all_lines.len());
        let selected: Vec<&str> = all_lines[start..].iter().take(limit).copied().collect();
        let lines_returned = selected.len() as u32;
        let has_more = start + selected.len() < all_lines.len();

        Ok(ToolResult {
            content: json!({
                "content": selected.join("\n"),
                "total_lines": total_lines,
                "offset": offset,
                "lines_returned": lines_returned,
                "has_more": has_more,
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
