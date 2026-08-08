use serde_json::json;
use std::io::Write;

use crate::tools::sandbox::validate_path_with_root;
use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolVerificationStatus,
};
use crate::tools::vfl::LockType;

pub struct EditFileTool;

#[async_trait::async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Make surgical edits to a file by finding exact text and replacing it. Use this instead of write_file when only part of a file needs to change."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(
            json!({"path": "src/main.rs", "edits": [{"old_text": "let x = 1;", "new_text": "let x = 2;"}]}),
        )
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path within working directory" },
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_text": { "type": "string", "description": "Exact text to find (must match once)" },
                            "new_text": { "type": "string", "description": "Replacement text" }
                        },
                        "required": ["old_text", "new_text"]
                    },
                    "description": "Ordered list of edits to apply"
                }
            },
            "required": ["path", "edits"]
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

        let edits = arguments
            .get("edits")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: edits".to_string(),
                retryable: false,
            })?;

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

        // Acquire VFL write lock (blocks silently if locked by another agent)
        let vfl_lock = context
            .vfl_registry
            .acquire(&resolved, LockType::Write, &context.agent_id)
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
                    "File too large for editing ({:.1}MB, max {}MB)",
                    file_size as f64 / (1024.0 * 1024.0),
                    MAX_FILE_READ_BYTES / (1024 * 1024)
                ),
                retryable: false,
            });
        }

        // Do all work inside a block so we can always release the lock
        let result: Result<(u32, String), ToolError> = (|| {
            let original = std::fs::read_to_string(&resolved).map_err(|e| ToolError {
                code: ToolErrorCode::PermissionDenied,
                message: format!("Failed to read file: {}", e),
                retryable: false,
            })?;

            // Apply edits in order to a working copy
            let mut working = original.clone();
            let mut edits_applied = 0u32;

            for (i, edit) in edits.iter().enumerate() {
                let old_text = edit
                    .get("old_text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError {
                        code: ToolErrorCode::InvalidInput,
                        message: format!("Edit {} missing old_text", i + 1),
                        retryable: false,
                    })?;

                let new_text = edit
                    .get("new_text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError {
                        code: ToolErrorCode::InvalidInput,
                        message: format!("Edit {} missing new_text", i + 1),
                        retryable: false,
                    })?;

                let match_count = working.matches(old_text).count();

                if match_count == 0 {
                    return Err(ToolError {
                        code: ToolErrorCode::InvalidInput,
                        message: format!(
                            "Edit {}: old_text not found in file. Text: '{}'",
                            i + 1,
                            old_text.chars().take(100).collect::<String>()
                        ),
                        retryable: false,
                    });
                }

                if match_count > 1 {
                    return Err(ToolError {
                        code: ToolErrorCode::InvalidInput,
                        message: format!(
                            "Edit {}: old_text matches {} times (must match exactly once). Text: '{}'",
                            i + 1,
                            match_count,
                            old_text.chars().take(100).collect::<String>()
                        ),
                        retryable: false,
                    });
                }

                working = working.replacen(old_text, new_text, 1);
                edits_applied += 1;
            }

            // Generate unified diff
            let diff = generate_diff(&original, &working);

            // Atomic write: temp file + rename
            let parent = resolved.parent().unwrap_or(&context.working_directory);
            let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to create temp file: {}", e),
                retryable: true,
            })?;

            temp.write_all(working.as_bytes()).map_err(|e| ToolError {
                code: ToolErrorCode::PermissionDenied,
                message: format!("Failed to write edited content: {}", e),
                retryable: false,
            })?;

            temp.persist(&resolved).map_err(|e| ToolError {
                code: ToolErrorCode::PermissionDenied,
                message: format!("Failed to persist edited file: {}", e),
                retryable: true,
            })?;

            Ok((edits_applied, diff))
        })();

        // Always release lock, regardless of success or failure
        vfl_lock.release().await;

        // Record self-write for file watch suppression (only if edit succeeded)
        if result.is_ok() {
            if let Some(ref app_state) = context.app_state {
                app_state
                    .file_watch_manager
                    .record_self_write(&context.agent_id, &resolved)
                    .await;
            }
        }

        let (edits_applied, diff) = result?;

        Ok(ToolResult {
            content: json!({
                "tool_result_status": "success",
                "path": resolved.display().to_string(),
                "edits_applied": edits_applied,
                "diff": diff,
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
                        "Applied {} edit{} to {}",
                        edits_applied,
                        if edits_applied == 1 { "" } else { "s" },
                        resolved.display()
                    )),
                },
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

/// Generate a simple unified diff between two strings.
fn generate_diff(original: &str, modified: &str) -> String {
    let orig_lines: Vec<&str> = original.lines().collect();
    let mod_lines: Vec<&str> = modified.lines().collect();

    let mut diff = String::new();
    let mut i = 0;
    let mut j = 0;

    while i < orig_lines.len() || j < mod_lines.len() {
        if i < orig_lines.len() && j < mod_lines.len() && orig_lines[i] == mod_lines[j] {
            i += 1;
            j += 1;
        } else {
            // Find the extent of the change
            // Simple approach: show removed then added lines until we re-sync
            let mut orig_end = i;
            let mut mod_end = j;

            // Look ahead for re-sync
            let mut found = false;
            'outer: for (oi, orig_line) in orig_lines
                .iter()
                .enumerate()
                .take(orig_lines.len().min(i + 20))
                .skip(i)
            {
                for (mj, mod_line) in mod_lines
                    .iter()
                    .enumerate()
                    .take(mod_lines.len().min(j + 20))
                    .skip(j)
                {
                    if orig_line == mod_line {
                        orig_end = oi;
                        mod_end = mj;
                        found = true;
                        break 'outer;
                    }
                }
            }

            if !found {
                orig_end = orig_lines.len();
                mod_end = mod_lines.len();
            }

            // Output context
            if i > 0 {
                diff.push_str(&format!(
                    "@@ -{},{} +{},{} @@\n",
                    i + 1,
                    orig_end - i,
                    j + 1,
                    mod_end - j
                ));
            }

            for line in &orig_lines[i..orig_end] {
                diff.push_str(&format!("-{}\n", line));
            }
            for line in &mod_lines[j..mod_end] {
                diff.push_str(&format!("+{}\n", line));
            }

            i = orig_end;
            j = mod_end;
        }
    }

    if diff.is_empty() {
        "No changes".to_string()
    } else {
        diff
    }
}
