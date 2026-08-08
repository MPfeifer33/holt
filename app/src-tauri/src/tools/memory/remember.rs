use serde_json::json;

use crate::memory::store::{MemoryCategory, MemorySource, MemoryTier, StoreInput};
use crate::tools::types::{
    Tool, ToolAuthorityClass, ToolContext, ToolError, ToolErrorCode, ToolExecutionReceipt,
    ToolResult, ToolResultOutcome, ToolVerificationStatus,
};

pub struct RememberTool;

#[async_trait::async_trait]
impl Tool for RememberTool {
    fn name(&self) -> &'static str {
        "remember"
    }

    fn description(&self) -> &'static str {
        "Save something to long-term memory so it survives across sessions. Use this for decisions, preferences, architectural choices, gotchas, or anything worth remembering later. Not for throwaway details."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(
            json!({"content": "Config uses port 8080 by default", "category": "concept", "importance": 0.7}),
        )
    }

    fn authority_class(&self) -> ToolAuthorityClass {
        ToolAuthorityClass::EffectfulVerified
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "What to remember — a clear, self-contained statement"
                },
                "category": {
                    "type": "string",
                    "enum": ["episode", "concept", "policy", "identity"],
                    "description": "Memory category: episode (what happened), concept (facts/knowledge), policy (rules/preferences), identity (sacred/who we are). Default: episode"
                },
                "importance": {
                    "type": ["number", "string"],
                    "minimum": 0.0,
                    "maximum": 1.0,
                    "description": "Importance from 0.0 to 1.0. Higher values are retrieved more readily. Default: 0.7"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tags for categorization and retrieval filtering"
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let app_state = context.app_state.as_ref().ok_or_else(|| ToolError {
            code: ToolErrorCode::InternalError,
            message: "AppState not available".to_string(),
            retryable: false,
        })?;

        let memory_engine = app_state.get_memory_engine().ok_or_else(|| ToolError {
            code: ToolErrorCode::InternalError,
            message: "Memory system not available".to_string(),
            retryable: false,
        })?;

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: content".to_string(),
                retryable: false,
            })?
            .to_string();

        let category =
            MemoryCategory::from_tool_arg(arguments.get("category").and_then(|v| v.as_str()));

        let importance = arguments
            .get("importance")
            .and_then(|v| match v {
                serde_json::Value::Number(n) => n.as_f64(),
                serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
                _ => None,
            })
            .unwrap_or(0.7)
            .clamp(0.0, 1.0);

        let agent_ns = crate::memory::MemoryEngine::agent_namespace(&context.agent_id);

        let tags: Vec<String> = arguments
            .get("tags")
            .map(|v| match v {
                serde_json::Value::Array(arr) => arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
                serde_json::Value::String(s) => s
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect(),
                _ => vec![],
            })
            .unwrap_or_default();

        let input = StoreInput {
            content: content.clone(),
            category,
            importance,
            source: MemorySource::AgentExplicit,
            agent_ns,
            tier: MemoryTier::Warm,
            tags,
        };

        let stored = memory_engine
            .store_detailed(input)
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to store memory: {e}"),
                retryable: true,
            })?;
        let verification_status = match stored.verification_status.as_str() {
            "verified" => ToolVerificationStatus::Verified,
            "verification_failed" => ToolVerificationStatus::VerificationFailed,
            "verification_skipped" => ToolVerificationStatus::VerificationSkipped,
            _ => ToolVerificationStatus::NotRequired,
        };
        let outcome = if matches!(
            verification_status,
            ToolVerificationStatus::VerificationFailed
        ) {
            ToolResultOutcome::Partial
        } else {
            ToolResultOutcome::Success
        };
        let summary = Some("Memory stored successfully".to_string());

        let receipt = ToolExecutionReceipt {
            authority_class: ToolAuthorityClass::EffectfulVerified,
            executed: true,
            execution_status: "success".to_string(),
            verified: matches!(verification_status, ToolVerificationStatus::Verified),
            verification_status,
            execution_id: None,
            tool_name: None,
            tool_call_id: None,
            tool_call_trace_id: None,
            tool_result_trace_id: None,
            summary,
        };

        Ok(ToolResult {
            content: json!({
                "stored": true,
                "memory_id": stored.memory_id,
                "status": stored.status,
                "message": stored.message,
                "verification_status": stored.verification_status,
                "tool_result_status": match outcome {
                    ToolResultOutcome::Partial => "partial",
                    ToolResultOutcome::Success => "success",
                },
                "receipt": receipt,
            }),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
