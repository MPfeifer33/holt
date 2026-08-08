use serde_json::json;

use crate::tools::sandbox::validate_path_with_root;
use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolVerificationStatus,
};
use crate::tools::vfl::LockType;

pub struct ReadFileTool;

#[async_trait::async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read a file's contents, optionally starting from a specific line. Use this to inspect code, config, or documentation before making changes."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"path": "src/main.rs", "limit": 50}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path (relative or absolute within working directory)" },
                "offset": { "type": "integer", "description": "Start reading from this line number (1-indexed)" },
                "limit": { "type": "integer", "description": "Max number of lines to return" }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let path_str = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: path".to_string(),
                retryable: false,
            })?;

        let offset = arguments
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let limit = arguments.get("limit").and_then(|v| v.as_u64());

        let resolved = validate_path_with_root(
            path_str,
            &context.working_directory,
            &context.workspace_root,
        )?;

        // Validate before acquiring lock to avoid lock leak on early return
        if !resolved.exists() {
            return Err(ToolError {
                code: ToolErrorCode::FileNotFound,
                message: format!("File not found: {}", path_str),
                retryable: false,
            });
        }

        if !resolved.is_file() {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!("'{}' is not a file", path_str),
                retryable: false,
            });
        }

        // Acquire VFL read lock (blocks silently if write-locked by another agent)
        let vfl_lock = context
            .vfl_registry
            .acquire(&resolved, LockType::Read, &context.agent_id)
            .await?;

        // File size guard: reject files over 50MB before reading into memory
        const MAX_FILE_READ_BYTES: u64 = 50 * 1024 * 1024; // 50MB
        let file_size = std::fs::metadata(&resolved)
            .map_err(|e| ToolError {
                code: ToolErrorCode::PermissionDenied,
                message: format!("Failed to read file metadata: {}", e),
                retryable: false,
            })?
            .len();
        if file_size > MAX_FILE_READ_BYTES {
            vfl_lock.release().await;
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!(
                    "File too large ({:.1}MB, max {}MB). Use offset/limit for partial reads.",
                    file_size as f64 / (1024.0 * 1024.0),
                    MAX_FILE_READ_BYTES / (1024 * 1024)
                ),
                retryable: false,
            });
        }

        // Read bytes and check for binary
        let bytes = std::fs::read(&resolved).map_err(|e| ToolError {
            code: ToolErrorCode::PermissionDenied,
            message: format!("Failed to read file: {}", e),
            retryable: false,
        })?;

        // Image file detection — return base64 content block instead of text
        let extension = resolved
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase());
        let image_extensions = ["png", "jpg", "jpeg", "gif", "webp"];
        if let Some(ref ext) = extension {
            if image_extensions.contains(&ext.as_str()) {
                // Size check: cap at 5MB
                if bytes.len() > 5_000_000 {
                    vfl_lock.release().await;
                    return Err(ToolError {
                        code: ToolErrorCode::InvalidInput,
                        message: format!(
                            "Image too large ({:.1}MB, max 5MB)",
                            bytes.len() as f64 / 1_000_000.0
                        ),
                        retryable: false,
                    });
                }

                use base64::Engine;
                let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let media_type = match ext.as_str() {
                    "png" => "image/png",
                    "jpg" | "jpeg" => "image/jpeg",
                    "gif" => "image/gif",
                    "webp" => "image/webp",
                    _ => "application/octet-stream",
                };

                vfl_lock.release().await;

                return Ok(ToolResult {
                    content: json!({
                        "tool_result_status": "success",
                        "type": "image",
                        "path": resolved.display().to_string(),
                        "media_type": media_type,
                        "size_bytes": bytes.len(),
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
                            summary: Some(format!("Read image file {}", resolved.display())),
                        },
                    }),
                    truncated: false,
                    trace_id: None,
                    image_content: Some(vec![crate::runtime::agent::ContentBlock::Image {
                        data,
                        media_type: media_type.to_string(),
                    }]),
                });
            }
        }

        // Binary detection: check first 512 bytes for null bytes
        let check_len = bytes.len().min(512);
        if bytes[..check_len].contains(&0) {
            vfl_lock.release().await;
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "File appears to be binary and cannot be read as text".to_string(),
                retryable: false,
            });
        }

        let content = String::from_utf8_lossy(&bytes);
        let all_lines: Vec<&str> = content.lines().collect();
        let total_lines = all_lines.len() as u32;

        // Apply offset (1-indexed → 0-indexed)
        let start = if offset > 0 {
            (offset - 1).min(all_lines.len())
        } else {
            0
        };
        let selected: Vec<&str> = if let Some(lim) = limit {
            all_lines[start..]
                .iter()
                .take(lim as usize)
                .copied()
                .collect()
        } else {
            all_lines[start..].to_vec()
        };

        let lines_returned = selected.len() as u32;
        let result_content = selected.join("\n");
        let truncated = result_content.len() > 50_000;

        let display = if truncated {
            let boundary = result_content.floor_char_boundary(50_000);
            format!("{}...\n\n[Output truncated]", &result_content[..boundary])
        } else {
            result_content
        };

        vfl_lock.release().await;

        Ok(ToolResult {
            content: json!({
                "tool_result_status": "success",
                "content": display,
                "total_lines": total_lines,
                "lines_returned": lines_returned,
                "truncated": truncated,
                "path": resolved.display().to_string(),
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
                    summary: Some(format!(
                        "Read {} line{} from {}",
                        lines_returned,
                        if lines_returned == 1 { "" } else { "s" },
                        resolved.display()
                    )),
                },
            }),
            truncated,
            trace_id: None,
            image_content: None,
        })
    }
}
