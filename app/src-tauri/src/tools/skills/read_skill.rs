use serde_json::json;

use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolSuccessBehavior, ToolVerificationStatus,
};

pub struct ReadSkillTool;

#[async_trait::async_trait]
impl Tool for ReadSkillTool {
    fn name(&self) -> &str {
        "read_skill"
    }

    fn description(&self) -> &str {
        "Load the full content of a skill from the Available Skills manifest for use in your reasoning."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"name": "code-review"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The skill name exactly as shown in the Available Skills manifest"
                }
            },
            "required": ["name"]
        })
    }

    fn authority_class(&self) -> ToolAuthorityClass {
        ToolAuthorityClass::Informational
    }

    fn success_behavior(&self) -> ToolSuccessBehavior {
        ToolSuccessBehavior::ContinueLoop
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let skill_name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: name".to_string(),
                retryable: false,
            })?;

        // Access the skill registry via app state
        let app_state = context.app_state.as_ref().ok_or_else(|| ToolError {
            code: ToolErrorCode::InternalError,
            message: "AppState not available".to_string(),
            retryable: false,
        })?;

        let skill = app_state
            .skill_registry
            .get(skill_name)
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!("Skill '{}' not found in registry", skill_name),
                retryable: false,
            })?;

        Ok(ToolResult {
            content: json!({
                "tool_result_status": "success",
                "skill_name": skill.frontmatter.name,
                "description": skill.frontmatter.description,
                "priority": format!("{:?}", skill.frontmatter.priority).to_lowercase(),
                "content": skill.body,
                "token_budget": skill.frontmatter.max_tokens,
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
                    summary: Some(format!("Loaded skill '{}' ({} chars)", skill.frontmatter.name, skill.body.len())),
                },
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
