use serde_json::json;
use std::io::Write;

use crate::tools::sandbox::validate_path_with_root;
use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolVerificationStatus,
};
use crate::tools::vfl::LockType;

pub struct WriteFileTool;

#[async_trait::async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Create a new file or completely overwrite an existing one. Use this for new files or full rewrites. For partial changes, use edit_file instead."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"path": "notes.md", "content": "# Notes\n\nTODO: update this file"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Target file path within working directory" },
                "content": { "type": "string", "description": "Full file content to write" },
                "create_directories": { "type": "boolean", "description": "Create parent dirs if needed (default: true)" }
            },
            "required": ["path", "content"]
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

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: content".to_string(),
                retryable: false,
            })?;

        let create_directories = arguments
            .get("create_directories")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        // Write size guard: reject writes over 50MB
        const MAX_FILE_WRITE_BYTES: usize = 50 * 1024 * 1024; // 50MB
        if content.len() > MAX_FILE_WRITE_BYTES {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!(
                    "Content too large for write ({:.1}MB, max {}MB)",
                    content.len() as f64 / (1024.0 * 1024.0),
                    MAX_FILE_WRITE_BYTES / (1024 * 1024)
                ),
                retryable: false,
            });
        }

        let resolved = validate_path_with_root(
            path_str,
            &context.working_directory,
            &context.workspace_root,
        )?;

        // Acquire VFL write lock (blocks silently if locked by another agent)
        let vfl_lock = context
            .vfl_registry
            .acquire(&resolved, LockType::Write, &context.agent_id)
            .await?;

        // Do all work inside a block so we can always release the lock
        let result: Result<(bool, u64), ToolError> = (|| {
            let created = !resolved.exists();

            // Create parent directories if needed
            if create_directories {
                if let Some(parent) = resolved.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| ToolError {
                        code: ToolErrorCode::PermissionDenied,
                        message: format!("Failed to create directories: {}", e),
                        retryable: false,
                    })?;
                }
            }

            // Atomic write: write to temp file, then rename
            let parent = resolved.parent().unwrap_or(&context.working_directory);
            let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to create temp file: {}", e),
                retryable: true,
            })?;

            temp.write_all(content.as_bytes()).map_err(|e| ToolError {
                code: ToolErrorCode::PermissionDenied,
                message: format!("Failed to write content: {}", e),
                retryable: false,
            })?;

            // Persist (rename) the temp file to the target path
            temp.persist(&resolved).map_err(|e| ToolError {
                code: ToolErrorCode::PermissionDenied,
                message: format!("Failed to persist file: {}", e),
                retryable: true,
            })?;

            Ok((created, content.len() as u64))
        })();

        // Always release lock, regardless of success or failure
        vfl_lock.release().await;

        // Record self-write for file watch suppression (only if write succeeded)
        if result.is_ok() {
            if let Some(ref app_state) = context.app_state {
                app_state
                    .file_watch_manager
                    .record_self_write(&context.agent_id, &resolved)
                    .await;
            }
        }

        let (created, bytes_written) = result?;

        Ok(ToolResult {
            content: json!({
                "tool_result_status": "success",
                "path": resolved.display().to_string(),
                "bytes_written": bytes_written,
                "created": created,
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
                    summary: Some(format!(
                        "{} {} ({} bytes)",
                        if created { "Created" } else { "Rewrote" },
                        resolved.display(),
                        bytes_written
                    )),
                },
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
