use serde_json::json;

use crate::tools::sandbox::validate_path_with_root;
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};
use crate::tools::vfl::LockType;

pub struct DeleteFileTool;

#[async_trait::async_trait]
impl Tool for DeleteFileTool {
    fn name(&self) -> &'static str {
        "delete_file"
    }

    fn description(&self) -> &'static str {
        "Delete a file or directory. Requires human approval by default."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"path": "tmp/old-output.log"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to delete within working directory" },
                "recursive": { "type": "boolean", "description": "Delete directory contents recursively (default: false)" }
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

        let recursive = arguments
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let resolved = validate_path_with_root(
            path_str,
            &context.working_directory,
            &context.workspace_root,
        )?;

        // Validate before acquiring locks to avoid lock leak on early return
        if !resolved.exists() {
            return Err(ToolError {
                code: ToolErrorCode::FileNotFound,
                message: format!("Path not found: {}", path_str),
                retryable: false,
            });
        }

        // Note: Approval is now handled at the engine level (approval gate in engine.rs)
        // before this tool's execute() is called. No inline approval check needed.

        // Acquire VFL write lock on file/dir (blocks silently if locked by another agent)
        let vfl_lock = context
            .vfl_registry
            .acquire(&resolved, LockType::Write, &context.agent_id)
            .await?;

        // Also acquire write lock on parent directory to prevent race conditions
        let vfl_parent_lock = if let Some(parent) = resolved.parent() {
            Some(
                context
                    .vfl_registry
                    .acquire(parent, LockType::Write, &context.agent_id)
                    .await?,
            )
        } else {
            None
        };

        // Do all work inside a block so we can always release locks
        let result: Result<(bool, u32), ToolError> = (|| {
            let was_directory = resolved.is_dir();
            let items_deleted;

            if was_directory {
                if recursive {
                    items_deleted = count_entries(&resolved);
                    std::fs::remove_dir_all(&resolved).map_err(|e| ToolError {
                        code: ToolErrorCode::PermissionDenied,
                        message: format!("Failed to delete directory: {}", e),
                        retryable: false,
                    })?;
                } else {
                    std::fs::remove_dir(&resolved).map_err(|e| ToolError {
                        code: ToolErrorCode::PermissionDenied,
                        message: format!(
                            "Failed to delete directory (not empty? use recursive=true): {}",
                            e
                        ),
                        retryable: false,
                    })?;
                    items_deleted = 1;
                }
            } else {
                std::fs::remove_file(&resolved).map_err(|e| ToolError {
                    code: ToolErrorCode::PermissionDenied,
                    message: format!("Failed to delete file: {}", e),
                    retryable: false,
                })?;
                items_deleted = 1;
            }

            Ok((was_directory, items_deleted))
        })();

        // Always release locks, regardless of success or failure
        vfl_lock.release().await;
        if let Some(parent_lock) = vfl_parent_lock {
            parent_lock.release().await;
        }

        // Record self-write for file watch suppression (only if delete succeeded)
        if result.is_ok() {
            if let Some(ref app_state) = context.app_state {
                app_state
                    .file_watch_manager
                    .record_self_write(&context.agent_id, &resolved)
                    .await;
            }
        }

        let (was_directory, items_deleted) = result?;

        Ok(ToolResult {
            content: json!({
                "path": resolved.display().to_string(),
                "was_directory": was_directory,
                "items_deleted": items_deleted,
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

fn count_entries(path: &std::path::Path) -> u32 {
    let mut count = 0u32;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            count += 1;
            if entry.path().is_dir() {
                count += count_entries(&entry.path());
            }
        }
    }
    count + 1 // Include the directory itself
}
