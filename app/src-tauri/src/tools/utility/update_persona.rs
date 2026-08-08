use async_trait::async_trait;
use serde_json::{json, Value};

use crate::runtime::persona::{
    affected_anchors_for_edit, emit_anchor_owner_notice, format_anchor_retry_message,
    load_anchor_manifest, validate_persona_filename,
};
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct UpdatePersonaTool;

#[async_trait]
impl Tool for UpdatePersonaTool {
    fn name(&self) -> &str {
        "update_persona"
    }

    fn description(&self) -> &str {
        "Update one of your persona files (e.g., SOUL.md, TOOLS.md). Writes directly to your persona directory."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"file": "TOOLS.md", "content": "# Tools\n\nUpdated tool list..."}))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "Persona filename (must end in .md, e.g. SOUL.md)"
                },
                "content": {
                    "type": "string",
                    "description": "Full file content to write"
                },
                "confirm_anchored_edit": {
                    "type": "boolean",
                    "description": "Required when the edit would change anchored content. Set to true only when you intend a high-friction anchor edit."
                },
                "reason": {
                    "type": "string",
                    "description": "Required when confirm_anchored_edit=true. Briefly explain why the anchored change is necessary."
                }
            },
            "required": ["file", "content"]
        })
    }

    async fn execute(
        &self,
        arguments: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let file = arguments
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing 'file' argument".to_string(),
                retryable: false,
            })?;

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing 'content' argument".to_string(),
                retryable: false,
            })?;

        if let Err(message) = validate_persona_filename(file) {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message,
                retryable: false,
            });
        }

        // Validation: content size (8KB limit per persona file)
        const MAX_PERSONA_FILE_SIZE: usize = 8 * 1024;
        if content.len() > MAX_PERSONA_FILE_SIZE {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!(
                    "Content is {} bytes, exceeds {} byte limit for persona files",
                    content.len(),
                    MAX_PERSONA_FILE_SIZE
                ),
                retryable: false,
            });
        }

        // Write to persona directory
        let persona_dir = crate::runtime::persona::persona_dir(&context.agent_id);
        if let Err(e) = std::fs::create_dir_all(&persona_dir) {
            return Err(ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to create persona directory: {}", e),
                retryable: true,
            });
        }

        let file_path = persona_dir.join(file);
        let old_content = std::fs::read_to_string(&file_path).unwrap_or_default();
        let manifest = load_anchor_manifest(&context.agent_id);
        let affected_anchors = affected_anchors_for_edit(&manifest, file, &old_content, content);

        let confirm_anchored_edit = arguments
            .get("confirm_anchored_edit")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let reason = arguments
            .get("reason")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());

        if !affected_anchors.is_empty() && (!confirm_anchored_edit || reason.is_none()) {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format_anchor_retry_message(file, &affected_anchors),
                retryable: false,
            });
        }

        std::fs::write(&file_path, content).map_err(|e| ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("Failed to write persona file: {}", e),
            retryable: true,
        })?;

        // Log identity change to PERSONA_CHANGELOG.md
        let changelog_summary = if !affected_anchors.is_empty() {
            format!("Updated {} (anchored edit)", file)
        } else {
            format!("Updated {}", file)
        };
        crate::runtime::persona::append_changelog(
            &context.agent_id,
            file,
            &changelog_summary,
            &crate::runtime::persona::ChangeInitiator::Agent,
        );

        if let Some(change_reason) = reason {
            if !affected_anchors.is_empty() {
                emit_anchor_owner_notice(
                    context.app_handle.as_ref(),
                    &context.agent_id,
                    file,
                    &affected_anchors,
                    change_reason,
                    "update_persona",
                );
            }
        }

        Ok(ToolResult {
            content: json!({
                "status": "ok",
                "path": file_path.to_string_lossy(),
                "bytes": content.len(),
                "anchored_edit": !affected_anchors.is_empty(),
                "affected_anchor_ids": affected_anchors
                    .iter()
                    .map(|anchor| anchor.anchor_id.clone())
                    .collect::<Vec<_>>()
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test validation logic directly without needing a full ToolContext.
    fn validate_persona_args(file: &str, content: &str) -> Result<(), ToolError> {
        if let Err(message) = validate_persona_filename(file) {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message,
                retryable: false,
            });
        }
        const MAX: usize = 8 * 1024;
        if content.len() > MAX {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!("Content too large: {} > {}", content.len(), MAX),
                retryable: false,
            });
        }
        Ok(())
    }

    #[test]
    fn test_valid_md_file() {
        assert!(validate_persona_args("SOUL.md", "content").is_ok());
        assert!(validate_persona_args("HEARTBEAT.md", "content").is_ok());
        assert!(validate_persona_args("TOOLS.md", "content").is_ok());
    }

    #[test]
    fn test_rejects_non_md_file() {
        let err = validate_persona_args("config.toml", "test").unwrap_err();
        assert_eq!(err.code, ToolErrorCode::InvalidInput);
    }

    #[test]
    fn test_rejects_path_traversal_dotdot() {
        let err = validate_persona_args("../../../etc/passwd.md", "test").unwrap_err();
        assert_eq!(err.code, ToolErrorCode::InvalidInput);
    }

    #[test]
    fn test_rejects_slash_in_filename() {
        let err = validate_persona_args("sub/SOUL.md", "test").unwrap_err();
        assert_eq!(err.code, ToolErrorCode::InvalidInput);
    }

    #[test]
    fn test_rejects_backslash_in_filename() {
        let err = validate_persona_args("sub\\SOUL.md", "test").unwrap_err();
        assert_eq!(err.code, ToolErrorCode::InvalidInput);
    }

    #[test]
    fn test_rejects_oversized_content() {
        let big = "x".repeat(9000); // > 8KB
        let err = validate_persona_args("SOUL.md", &big).unwrap_err();
        assert_eq!(err.code, ToolErrorCode::InvalidInput);
    }

    #[test]
    fn test_accepts_max_size_content() {
        let exactly_8k = "x".repeat(8 * 1024);
        assert!(validate_persona_args("SOUL.md", &exactly_8k).is_ok());
    }

    #[test]
    fn test_tool_metadata() {
        let tool = UpdatePersonaTool;
        assert_eq!(tool.name(), "update_persona");
        assert!(tool.description().contains("persona"));
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["file"].is_object());
        assert!(schema["properties"]["content"].is_object());
        assert!(schema["properties"]["confirm_anchored_edit"].is_object());
        assert!(schema["properties"]["reason"].is_object());
    }
}
