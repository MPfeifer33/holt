use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct RecallTool;

#[async_trait::async_trait]
impl Tool for RecallTool {
    fn name(&self) -> &'static str {
        "recall"
    }

    fn description(&self) -> &'static str {
        "Search your long-term memory by natural language query. Returns the most relevant stored memories. Use this to recover prior decisions, facts, or context from previous sessions."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"query": "what port does the server use", "limit": 5}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language search query"
                },
                "limit": {
                    "type": ["integer", "string"],
                    "description": "Maximum number of results to return (default: 5)"
                },
                "category": {
                    "type": "string",
                    "enum": ["episode", "concept", "policy", "identity"],
                    "description": "Filter results to a specific category"
                },
                "spaces": {
                    "type": "string",
                    "enum": ["private", "commons", "all"],
                    "description": "Which memory spaces to search. Default: private (your own memories only). Use 'all' to include shared commons memories."
                }
            },
            "required": ["query"]
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

        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: query".to_string(),
                retryable: false,
            })?
            .to_string();

        let limit = arguments
            .get("limit")
            .and_then(|v| match v {
                serde_json::Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f as u64)),
                serde_json::Value::String(s) => s.trim().parse::<u64>().ok(),
                _ => None,
            })
            .unwrap_or(5) as usize;

        let category = arguments
            .get("category")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let spaces = match arguments.get("spaces").and_then(|v| v.as_str()) {
            Some("all") => Some(hillock_core::model::SpaceScope::All),
            Some("commons") => Some(hillock_core::model::SpaceScope::Commons),
            _ => None, // Private is the default inside retrieve()
        };

        let agent_ns = crate::memory::MemoryEngine::agent_namespace(&context.agent_id);

        let result = memory_engine
            .retrieve(
                &query,
                limit,
                &agent_ns,
                None,
                category.as_deref(),
                None,
                spaces,
            )
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to retrieve memories: {e}"),
                retryable: true,
            })?;

        let memories: Vec<serde_json::Value> = result
            .memories
            .iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "content": r.content,
                    "score": format!("{:.3}", r.score),
                    "tier": r.tier,
                    "category": r.category,
                    "importance": r.importance,
                    "pinned": r.pinned,
                    "created_at": r.created_at,
                })
            })
            .collect();

        let mut response = json!({
            "memories": memories,
            "count": memories.len(),
        });

        // Transparency & ergonomics: pass through envelope metadata from Hillock
        if let Some(nm) = &result.near_misses {
            response["near_misses"] = nm.clone();
        }
        if let Some(rc) = &result.retrieval_confidence {
            response["retrieval_confidence"] = rc.clone();
        }
        if let Some(td) = &result.temporal_distribution {
            response["temporal_distribution"] = td.clone();
        }
        if let Some(rm) = &result.result_mix {
            response["result_mix"] = rm.clone();
        }
        if let Some(ri) = &result.retrieval_interpretation {
            response["retrieval_interpretation"] = ri.clone();
        }
        if let Some(rf) = &result.recommended_followup {
            // Remap Hillock tool names to Holt-exposed names
            let mut followup = rf.clone();
            if let Some(tool) = followup.get("tool").and_then(|t| t.as_str()) {
                let remapped = match tool {
                    "memory_recall" => "recall",
                    "memory_ingest" => "remember",
                    // memory_filter is the same on both sides
                    other => other,
                };
                followup["tool"] = serde_json::Value::String(remapped.to_string());
            }
            response["recommended_followup"] = followup;
        }
        if let Some(hl) = &result.highlights {
            response["highlights"] = hl.clone();
        }

        Ok(ToolResult {
            content: response,
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
