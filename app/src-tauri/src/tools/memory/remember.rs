use serde_json::json;

use crate::memory::store::{MemoryCategory, MemorySource, MemoryTier, StoreInput, WriteOptions};
use crate::memory::types::HillockStoreResult;
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
        "Save something to long-term memory so it survives across sessions. Use this for decisions, preferences, architectural choices, gotchas, or anything worth remembering later. Not for throwaway details. The result says whether the memory was created or was already known, and lists up to three existing memories that are similar; if one of them is now out of date, save again with its id in `supersedes`."
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
                },
                "supersedes": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Ids of memories this one replaces. Use it to update something in place: the old memories stop appearing in recall (they are kept, not deleted) and this one takes over."
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

        let options = WriteOptions {
            supersedes: supersedes_arg(&arguments),
        };

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
            .store_with(input, &options)
            .await
            .map_err(|e| match e {
                crate::memory::MemoryError::Validation(reason) => ToolError {
                    code: ToolErrorCode::InvalidInput,
                    message: reason,
                    retryable: false,
                },
                e => ToolError {
                    code: ToolErrorCode::InternalError,
                    message: format!("Failed to store memory: {e}"),
                    retryable: true,
                },
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
        let summary = Some(stored.message.clone());

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

        let mut content = stored_json(&stored);
        content["verification_status"] = json!(stored.verification_status);
        content["tool_result_status"] = json!(match outcome {
            ToolResultOutcome::Partial => "partial",
            ToolResultOutcome::Success => "success",
        });
        content["receipt"] = json!(receipt);

        Ok(ToolResult {
            content,
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

/// The `supersedes` argument: an array of ids, or one comma-separated string.
fn supersedes_arg(arguments: &serde_json::Value) -> Vec<String> {
    match arguments.get("supersedes") {
        Some(serde_json::Value::Array(ids)) => ids
            .iter()
            .filter_map(|v| v.as_str())
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect(),
        Some(serde_json::Value::String(ids)) => ids
            .split(',')
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// What the write did: `created` or `already_known`, the id either way, and the nearest
/// existing memories when there are any.
fn stored_json(stored: &HillockStoreResult) -> serde_json::Value {
    let mut content = json!({
        "stored": true,
        "memory_id": stored.memory_id,
        "status": stored.status,
        "message": stored.message,
    });
    if !stored.similar.is_empty() {
        content["similar"] = stored
            .similar
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "similarity": format!("{:.3}", s.similarity),
                    "preview": s.preview,
                })
            })
            .collect();
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::SimilarMemory;

    fn stored(status: &str, similar: Vec<SimilarMemory>) -> HillockStoreResult {
        HillockStoreResult {
            memory_id: "id-1".into(),
            status: status.into(),
            message: "m".into(),
            verification_status: "not_required".into(),
            similar,
        }
    }

    #[test]
    fn the_result_carries_the_status_and_omits_an_empty_similar_list() {
        let j = stored_json(&stored("already_known", vec![]));
        assert_eq!(j["status"], "already_known");
        assert_eq!(j["memory_id"], "id-1");
        assert!(j.get("similar").is_none(), "{j}");
    }

    #[test]
    fn similar_memories_are_listed_with_id_similarity_and_preview() {
        let near = SimilarMemory {
            id: "id-2".into(),
            similarity: 0.9123,
            preview: "port 8080".into(),
        };
        let j = stored_json(&stored("created", vec![near]));
        assert_eq!(
            j["similar"][0],
            json!({"id": "id-2", "similarity": "0.912", "preview": "port 8080"})
        );
    }

    #[test]
    fn supersedes_is_an_optional_list_of_ids() {
        let schema = RememberTool.parameters_schema();
        assert_eq!(schema["properties"]["supersedes"]["type"], "array");
        assert!(!schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("supersedes")));

        assert_eq!(
            supersedes_arg(&json!({"supersedes": ["a", " b ", ""]})),
            vec!["a", "b"]
        );
        assert_eq!(
            supersedes_arg(&json!({"supersedes": "a, b"})),
            vec!["a", "b"]
        );
        assert!(supersedes_arg(&json!({"content": "c"})).is_empty());
    }
}
