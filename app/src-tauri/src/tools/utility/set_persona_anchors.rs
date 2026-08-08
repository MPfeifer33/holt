use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::HashSet;

use crate::runtime::persona::{
    anchor_id_for_scope, load_anchor_manifest, save_anchor_manifest, validate_persona_filename,
    PersonaAnchor, PersonaAnchorFrictionPolicy, PersonaAnchorManifest, PersonaAnchorScope,
};
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct SetPersonaAnchorsTool;

#[async_trait]
impl Tool for SetPersonaAnchorsTool {
    fn name(&self) -> &str {
        "set_persona_anchors"
    }

    fn description(&self) -> &str {
        "Declare constitutional anchors — load-bearing commitments like trust contracts or identity boundaries that require high-friction edits."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(
            json!({"anchors": [{"file": "SOUL.md", "scope_type": "file", "reason": "Core identity"}]}),
        )
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "anchors": {
                    "type": "array",
                    "description": "Full replacement set of your declared persona anchors. Pass [] to clear all anchors.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "file": {
                                "type": "string",
                                "description": "Persona filename ending in .md, e.g. SOUL.md"
                            },
                            "scope_type": {
                                "type": "string",
                                "enum": ["file", "markdown_heading"],
                                "description": "Anchor the whole file or a single markdown heading section"
                            },
                            "heading": {
                                "type": "string",
                                "description": "Required when scope_type=markdown_heading. Exact heading text without leading # characters."
                            },
                            "reason": {
                                "type": "string",
                                "description": "Why this scope is load-bearing and should require high-friction edits"
                            }
                        },
                        "required": ["file", "scope_type", "reason"]
                    }
                }
            },
            "required": ["anchors"]
        })
    }

    async fn execute(
        &self,
        arguments: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let anchors = arguments
            .get("anchors")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing 'anchors' array".to_string(),
                retryable: false,
            })?;

        let existing = load_anchor_manifest(&context.agent_id);
        let now = Utc::now().to_rfc3339();
        let mut declared = Vec::with_capacity(anchors.len());
        let mut seen_anchor_ids = HashSet::new();

        for anchor in anchors {
            let parsed = parse_anchor(anchor, context, &existing, &now)?;
            if !seen_anchor_ids.insert(parsed.anchor_id.clone()) {
                return Err(invalid_input(&format!(
                    "Duplicate anchor scope declared for '{}'",
                    parsed.anchor_id
                )));
            }
            declared.push(parsed);
        }

        let manifest = PersonaAnchorManifest {
            version: 1,
            agent_id: context.agent_id.clone(),
            updated_at: Some(now),
            updated_at_session: Some(context.session_id.clone()),
            anchors: declared.clone(),
        };

        save_anchor_manifest(&context.agent_id, &manifest).map_err(|e| ToolError {
            code: ToolErrorCode::InternalError,
            message: format!("Failed to save anchors manifest: {e}"),
            retryable: true,
        })?;

        Ok(ToolResult {
            content: json!({
                "status": "ok",
                "anchors_count": declared.len(),
                "anchor_ids": declared.iter().map(|anchor| anchor.anchor_id.clone()).collect::<Vec<_>>(),
                "path": crate::runtime::persona::anchors_manifest_path(&context.agent_id).to_string_lossy(),
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

fn parse_anchor(
    anchor: &Value,
    context: &ToolContext,
    existing: &PersonaAnchorManifest,
    now: &str,
) -> Result<PersonaAnchor, ToolError> {
    let file = anchor
        .get("file")
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid_input("Anchor is missing 'file'"))?
        .trim()
        .to_string();
    validate_persona_filename(&file).map_err(|message| ToolError {
        code: ToolErrorCode::InvalidInput,
        message,
        retryable: false,
    })?;

    let scope_type = anchor
        .get("scope_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid_input("Anchor is missing 'scope_type'"))?;

    let reason = anchor
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_input("Anchor is missing a non-empty 'reason'"))?
        .to_string();

    let scope = match scope_type {
        "file" => PersonaAnchorScope::File { file: file.clone() },
        "markdown_heading" => {
            let heading = anchor
                .get("heading")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    invalid_input(
                        "Anchor with scope_type=markdown_heading requires a non-empty 'heading'",
                    )
                })?
                .to_string();

            PersonaAnchorScope::MarkdownHeading {
                file: file.clone(),
                heading,
            }
        }
        other => {
            return Err(invalid_input(&format!(
                "Unsupported scope_type '{other}'. Expected 'file' or 'markdown_heading'"
            )));
        }
    };

    let anchor_id = anchor_id_for_scope(&scope);
    let existing_anchor = existing
        .anchors
        .iter()
        .find(|candidate| candidate.anchor_id == anchor_id);

    Ok(PersonaAnchor {
        anchor_id,
        declared_by: context.agent_id.clone(),
        scope,
        reason,
        friction_policy: PersonaAnchorFrictionPolicy::default(),
        created_at: existing_anchor
            .and_then(|candidate| candidate.created_at.clone())
            .or_else(|| Some(now.to_string())),
        updated_at: Some(now.to_string()),
        declared_at_session: Some(context.session_id.clone()),
    })
}

fn invalid_input(message: &str) -> ToolError {
    ToolError {
        code: ToolErrorCode::InvalidInput,
        message: message.to_string(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata_is_stable() {
        let tool = SetPersonaAnchorsTool;
        assert_eq!(tool.name(), "set_persona_anchors");
        assert!(tool.description().contains("constitutional anchors"));
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["anchors"].is_object());
    }
}
