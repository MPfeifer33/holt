use chrono::{DateTime, Utc};
use serde_json::json;

use crate::tools::sandbox::validate_path_with_root;
use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolVerificationStatus,
};
use crate::tools::vfl::LockType;

pub struct ListDirectoryTool;

#[async_trait::async_trait]
impl Tool for ListDirectoryTool {
    fn name(&self) -> &'static str {
        "list_directory"
    }

    fn description(&self) -> &'static str {
        "List files and subdirectories in a directory. Returns directories first, then files, sorted alphabetically. Use this to explore project structure."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"path": "src/", "recursive": false}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory path (default: working directory)" },
                "recursive": { "type": "boolean", "description": "Include subdirectories recursively (default: false)" },
                "include_hidden": { "type": "boolean", "description": "Include dotfiles/dotdirs (default: false)" }
            }
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
            .unwrap_or(".");

        let recursive = arguments
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let include_hidden = arguments
            .get("include_hidden")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let resolved = validate_path_with_root(
            path_str,
            &context.working_directory,
            &context.workspace_root,
        )?;

        // Validate before acquiring lock to avoid lock leak on early return
        if !resolved.is_dir() {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!("'{}' is not a directory", path_str),
                retryable: false,
            });
        }

        // Acquire VFL read lock on directory (blocks silently if write-locked by another agent)
        let vfl_lock = context
            .vfl_registry
            .acquire(&resolved, LockType::Read, &context.agent_id)
            .await?;

        let mut entries = Vec::new();
        collect_entries(
            &resolved,
            &resolved,
            recursive,
            include_hidden,
            &mut entries,
        )?;

        // Sort: directories first, then files, alphabetical within each group
        entries.sort_by(|a, b| {
            let a_is_dir = a.get("entry_type").and_then(|v| v.as_str()) == Some("directory");
            let b_is_dir = b.get("entry_type").and_then(|v| v.as_str()) == Some("directory");
            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => {
                    let a_name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let b_name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    a_name.cmp(b_name)
                }
            }
        });

        let total_entries = entries.len() as u32;
        let truncated = total_entries > 1000;
        if truncated {
            entries.truncate(1000);
        }

        vfl_lock.release().await;

        Ok(ToolResult {
            content: json!({
                "tool_result_status": "success",
                "path": resolved.display().to_string(),
                "entries": entries,
                "total_entries": total_entries,
                "truncated": truncated,
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
                        "Listed {} entr{} in {}",
                        total_entries,
                        if total_entries == 1 { "y" } else { "ies" },
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

fn collect_entries(
    base: &std::path::Path,
    dir: &std::path::Path,
    recursive: bool,
    include_hidden: bool,
    entries: &mut Vec<serde_json::Value>,
) -> Result<(), ToolError> {
    let read = std::fs::read_dir(dir).map_err(|e| ToolError {
        code: ToolErrorCode::PermissionDenied,
        message: format!("Failed to read directory: {}", e),
        retryable: false,
    })?;

    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files unless requested
        if !include_hidden && name.starts_with('.') {
            continue;
        }

        let metadata = entry.metadata().ok();
        let is_dir = metadata.as_ref().is_some_and(|m| m.is_dir());

        let relative_name = if dir == base {
            name.clone()
        } else {
            entry
                .path()
                .strip_prefix(base)
                .unwrap_or(&entry.path())
                .display()
                .to_string()
        };

        let size_bytes = if is_dir {
            serde_json::Value::Null
        } else {
            metadata
                .as_ref()
                .map_or(serde_json::Value::Null, |m| json!(m.len()))
        };

        let modified = metadata
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                let dt: DateTime<Utc> = t.into();
                dt.to_rfc3339()
            })
            .unwrap_or_default();

        entries.push(json!({
            "name": relative_name,
            "entry_type": if is_dir { "directory" } else { "file" },
            "size_bytes": size_bytes,
            "modified": modified,
        }));

        if recursive && is_dir {
            collect_entries(base, &entry.path(), recursive, include_hidden, entries)?;
        }
    }

    Ok(())
}
