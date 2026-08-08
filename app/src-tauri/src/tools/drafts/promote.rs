use serde_json::json;

use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolVerificationStatus,
};

pub struct DraftPromoteTool;

fn should_resolve_to_persona_dir(
    destination: &str,
    manifest: &crate::runtime::persona::PersonaAnchorManifest,
) -> bool {
    let destination_path = std::path::Path::new(destination);
    let bare_persona_filename = destination_path.components().count() == 1
        && destination_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some();
    if !bare_persona_filename {
        return false;
    }

    let destination_filename = destination_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    if crate::runtime::persona::validate_persona_filename(destination_filename).is_err() {
        return false;
    }

    crate::runtime::persona::is_standard_persona_filename(destination_filename)
        || manifest
            .anchors
            .iter()
            .any(|anchor| anchor.scope.file() == destination_filename)
}

#[async_trait::async_trait]
impl Tool for DraftPromoteTool {
    fn name(&self) -> &'static str {
        "draft_promote"
    }

    fn description(&self) -> &'static str {
        "Copy a finished draft to its final file path and optionally delete the draft."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"name": "research-notes", "destination": "docs/research.md"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Draft name to promote"
                },
                "destination": {
                    "type": "string",
                    "description": "Final file path (relative to working directory or absolute)"
                },
                "delete_draft": {
                    "type": "boolean",
                    "description": "Delete the draft after promotion. Default: true"
                },
                "confirm_anchored_edit": {
                    "type": "boolean",
                    "description": "Required when promoting into anchored persona content. Set to true only when you intend a high-friction anchor edit."
                },
                "reason": {
                    "type": "string",
                    "description": "Required when confirm_anchored_edit=true. Briefly explain why the anchored change is necessary."
                }
            },
            "required": ["name", "destination"]
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

        let destination = arguments
            .get("destination")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: destination".into(),
                retryable: false,
            })?;

        let delete_draft = arguments
            .get("delete_draft")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let confirm_anchored_edit = arguments
            .get("confirm_anchored_edit")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let reason = arguments
            .get("reason")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());

        // Read draft
        let filename = super::sanitize_name(name);
        let draft_path = super::drafts_dir(&context.agent_id).join(&filename);

        let content = std::fs::read_to_string(&draft_path).map_err(|e| ToolError {
            code: ToolErrorCode::InvalidInput,
            message: format!("Draft '{}' not found: {}", filename, e),
            retryable: false,
        })?;

        let destination_path = std::path::Path::new(destination);
        let destination_filename = destination_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let manifest = crate::runtime::persona::load_anchor_manifest(&context.agent_id);
        let resolve_to_persona_dir = should_resolve_to_persona_dir(destination, &manifest);

        // Resolve destination path
        let dest_path = if destination_path.is_absolute() {
            std::path::PathBuf::from(destination)
        } else if resolve_to_persona_dir {
            crate::runtime::persona::persona_dir(&context.agent_id).join(destination_filename)
        } else {
            std::path::PathBuf::from(&context.working_directory).join(destination)
        };

        // Validate destination is within working directory or persona directory
        let working_dir = std::path::Path::new(&context.working_directory)
            .canonicalize()
            .ok();
        let persona_dir = crate::config::app_config::agent_dir(&context.agent_id)
            .join("persona")
            .canonicalize()
            .ok();

        if let Some(ref wd) = working_dir {
            if let Some(parent) = dest_path.parent() {
                if let Ok(canon_dest) = parent.canonicalize() {
                    if !canon_dest.starts_with(wd)
                        && !persona_dir
                            .as_ref()
                            .is_some_and(|pd| canon_dest.starts_with(pd))
                    {
                        return Err(ToolError {
                            code: ToolErrorCode::InvalidInput,
                            message: "Destination must be within the agent's working directory or persona directory".into(),
                            retryable: false,
                        });
                    }
                }
            }
        }

        let persona_file =
            crate::runtime::persona::persona_filename_from_path(&context.agent_id, &dest_path);
        let old_content = std::fs::read_to_string(&dest_path).unwrap_or_default();
        let affected_anchors = if let Some(ref file) = persona_file {
            crate::runtime::persona::affected_anchors_for_edit(
                &manifest,
                file,
                &old_content,
                &content,
            )
        } else {
            Vec::new()
        };

        if !affected_anchors.is_empty() && (!confirm_anchored_edit || reason.is_none()) {
            let file = persona_file.as_deref().unwrap_or("persona file");
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: crate::runtime::persona::format_anchor_retry_message(
                    file,
                    &affected_anchors,
                ),
                retryable: false,
            });
        }

        // Create parent directories
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to create destination directory: {e}"),
                retryable: false,
            })?;
        }

        // Write to destination
        std::fs::write(&dest_path, &content).map_err(|e| ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("Failed to write to destination: {e}"),
            retryable: false,
        })?;

        if let (Some(file), Some(change_reason)) = (persona_file.as_deref(), reason) {
            if !affected_anchors.is_empty() {
                crate::runtime::persona::emit_anchor_owner_notice(
                    context.app_handle.as_ref(),
                    &context.agent_id,
                    file,
                    &affected_anchors,
                    change_reason,
                    "draft_promote",
                );
            }
        }

        // Delete draft if requested
        if delete_draft {
            let _ = std::fs::remove_file(&draft_path);
        }

        Ok(ToolResult {
            content: json!({
                "tool_result_status": "success",
                "success": true,
                "name": filename,
                "destination": dest_path.to_string_lossy(),
                "draft_deleted": delete_draft,
                "anchored_edit": !affected_anchors.is_empty(),
                "affected_anchor_ids": affected_anchors
                    .iter()
                    .map(|anchor| anchor.anchor_id.clone())
                    .collect::<Vec<_>>(),
                "message": "Draft promoted to final location",
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
                        "Promoted draft {} to {}",
                        filename,
                        dest_path.to_string_lossy()
                    )),
                }
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::persona::{
        PersonaAnchor, PersonaAnchorFrictionPolicy, PersonaAnchorManifest, PersonaAnchorScope,
    };

    use super::should_resolve_to_persona_dir;

    #[test]
    fn bare_standard_persona_filename_resolves_to_persona_dir() {
        assert!(should_resolve_to_persona_dir(
            "USER.md",
            &PersonaAnchorManifest::default()
        ));
        assert!(!should_resolve_to_persona_dir(
            "README.md",
            &PersonaAnchorManifest::default()
        ));
    }

    #[test]
    fn bare_anchored_custom_filename_resolves_to_persona_dir() {
        let manifest = PersonaAnchorManifest {
            version: 1,
            agent_id: "nix".into(),
            updated_at: None,
            updated_at_session: None,
            anchors: vec![PersonaAnchor {
                anchor_id: "file:COVENANT.md".into(),
                declared_by: "nix".into(),
                scope: PersonaAnchorScope::File {
                    file: "COVENANT.md".into(),
                },
                reason: "Load-bearing".into(),
                friction_policy: PersonaAnchorFrictionPolicy::default(),
                created_at: None,
                updated_at: None,
                declared_at_session: None,
            }],
        };

        assert!(should_resolve_to_persona_dir("COVENANT.md", &manifest));
    }
}
