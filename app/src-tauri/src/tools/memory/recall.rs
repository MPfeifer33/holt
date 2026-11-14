use serde_json::json;

use crate::memory::types::RecalledMemory;
use crate::memory::{CapturedRows, RecallMode};
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct RecallTool;

#[async_trait::async_trait]
impl Tool for RecallTool {
    fn name(&self) -> &'static str {
        "recall"
    }

    fn description(&self) -> &'static str {
        "Search your long-term memory by natural language query. Returns the most relevant stored memories. A long memory is matched by fragment: the result then shows that fragment's text, and recall with memory_id=<its id> returns the whole memory. Use this to recover prior decisions, facts, or context from previous sessions."
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
                    "description": "Natural language search query. Give exactly one of query or memory_id."
                },
                "memory_id": {
                    "type": "string",
                    "description": "Fetch one memory by its id, in full, instead of searching. Use it to read the whole memory behind a fragment result."
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
                "include_captured": {
                    "type": "boolean",
                    "description": "Also search the raw conversation turns and session notes that were captured automatically. Default false, because they outnumber and crowd out the memories you saved on purpose. Set true only when you need the exact wording of an old conversation."
                },
                "spaces": {
                    "type": "string",
                    "enum": ["private", "commons", "all"],
                    "description": "Which memory spaces to search. Default: private (your own memories only). Use 'all' to include shared commons memories."
                }
            },
            "required": []
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

        let target = recall_target(&arguments)?;

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

        let query = match target {
            RecallTarget::Memory(id) => {
                let memory = memory_engine
                    .fetch(&id, &agent_ns)
                    .await
                    .map_err(|e| ToolError {
                        code: ToolErrorCode::InvalidInput,
                        message: format!("{e}"),
                        retryable: false,
                    })?;
                return Ok(ToolResult {
                    content: json!({ "memory": fetched_json(&memory) }),
                    truncated: false,
                    trace_id: None,
                    image_content: None,
                });
            }
            RecallTarget::Search(query) => query,
        };

        // The agent called this on purpose, so what comes back is reinforced.
        let mode = RecallMode::deliberate(if include_captured(&arguments) {
            CapturedRows::Include
        } else {
            CapturedRows::Exclude
        });
        let result = memory_engine
            .retrieve(
                &query,
                limit,
                &agent_ns,
                None,
                category.as_deref(),
                mode,
                spaces,
            )
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to retrieve memories: {e}"),
                retryable: true,
            })?;

        let memories: Vec<serde_json::Value> = result.memories.iter().map(memory_json).collect();

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

/// What one recall call asks for.
#[derive(Debug, PartialEq, Eq)]
enum RecallTarget {
    Search(String),
    Memory(String),
}

/// Exactly one of `query` and `memory_id`. Blank strings count as absent.
fn recall_target(arguments: &serde_json::Value) -> Result<RecallTarget, ToolError> {
    let given = |name: &str| {
        arguments
            .get(name)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let invalid = |message: &str| ToolError {
        code: ToolErrorCode::InvalidInput,
        message: message.to_string(),
        retryable: false,
    };
    match (given("query"), given("memory_id")) {
        (Some(q), None) => Ok(RecallTarget::Search(q)),
        (None, Some(id)) => Ok(RecallTarget::Memory(id)),
        (None, None) => Err(invalid(
            "Give one of: query (search) or memory_id (fetch one memory in full)",
        )),
        (Some(_), Some(_)) => Err(invalid("Give only one of query and memory_id, not both")),
    }
}

fn include_captured(arguments: &serde_json::Value) -> bool {
    match arguments.get("include_captured") {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => s.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// One search hit. `id` is always the memory `pin_memory` and `forget` accept; when the
/// hit is a fragment of a long memory, the fragment's own id and position ride along.
fn memory_json(r: &RecalledMemory) -> serde_json::Value {
    let mut m = json!({
        "id": r.memory_id(),
        "content": r.content,
        "score": format!("{:.3}", r.score),
        "cosine": format!("{:.3}", r.cosine),
        "category": r.category,
        "importance": r.importance,
        "pinned": r.pinned,
        "created_at": r.created_at,
    });
    if r.parent_id.is_some() {
        m["fragment_id"] = json!(r.id);
        m["chunk_index"] = json!(r.chunk_index);
        m["chunk_count"] = json!(r.chunk_count);
    }
    if r.superseded {
        m["superseded"] = json!(true);
    }
    m
}

/// A memory fetched by id. Nothing was searched, so there is no score to show.
fn fetched_json(r: &RecalledMemory) -> serde_json::Value {
    let mut m = memory_json(r);
    if let Some(fields) = m.as_object_mut() {
        fields.remove("score");
        fields.remove("cosine");
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recalled() -> RecalledMemory {
        RecalledMemory {
            id: "11111111-0000-4000-8000-000000000001".into(),
            parent_id: None,
            chunk_index: None,
            chunk_count: None,
            content: "the server listens on 8080".into(),
            category: "concept".into(),
            importance: 0.7,
            score: 0.8123,
            cosine: 0.7456,
            tags: vec![],
            source: String::new(),
            source_type: None,
            created_at: "2026-05-03T09:00:00+00:00".into(),
            pinned: true,
            superseded: false,
        }
    }

    #[test]
    fn a_whole_memory_hit_shows_both_scores_and_no_tier() {
        let j = memory_json(&recalled());
        assert_eq!(j["id"], "11111111-0000-4000-8000-000000000001");
        assert_eq!(j["score"], "0.812");
        assert_eq!(j["cosine"], "0.746");
        assert_eq!(j["pinned"], true);
        for absent in [
            "tier",
            "fragment_id",
            "chunk_index",
            "chunk_count",
            "superseded",
        ] {
            assert!(
                j.get(absent).is_none(),
                "{absent} should not be printed: {j}"
            );
        }
    }

    #[test]
    fn a_fragment_hit_is_addressed_by_its_parent() {
        let parent = "22222222-0000-4000-8000-000000000002";
        let m = RecalledMemory {
            parent_id: Some(parent.into()),
            chunk_index: Some(1),
            chunk_count: Some(3),
            ..recalled()
        };
        let j = memory_json(&m);
        assert_eq!(
            j["id"], parent,
            "pin_memory and forget take the parent's id"
        );
        assert_eq!(j["fragment_id"], "11111111-0000-4000-8000-000000000001");
        assert_eq!(
            (j["chunk_index"].as_u64(), j["chunk_count"].as_u64()),
            (Some(1), Some(3))
        );
    }

    #[test]
    fn a_fetched_memory_carries_no_search_scores() {
        let j = fetched_json(&RecalledMemory {
            superseded: true,
            ..recalled()
        });
        assert!(j.get("score").is_none() && j.get("cosine").is_none(), "{j}");
        assert_eq!(j["content"], "the server listens on 8080");
        assert_eq!(j["superseded"], true);
    }

    #[test]
    fn recall_takes_exactly_one_of_query_and_memory_id() {
        let target = |args: serde_json::Value| recall_target(&args);
        assert_eq!(
            target(json!({"query": " ports "})).unwrap(),
            RecallTarget::Search("ports".into())
        );
        assert_eq!(
            target(json!({"memory_id": "abc", "query": "  "})).unwrap(),
            RecallTarget::Memory("abc".into())
        );

        let neither = target(json!({"limit": 5})).unwrap_err();
        assert!(
            neither.message.contains("query") && neither.message.contains("memory_id"),
            "{}",
            neither.message
        );
        let both = target(json!({"query": "q", "memory_id": "abc"})).unwrap_err();
        assert!(both.message.contains("only one"), "{}", both.message);
    }

    #[test]
    fn captured_turns_are_searched_only_on_request() {
        assert!(!include_captured(&json!({"query": "q"})));
        assert!(!include_captured(&json!({"include_captured": false})));
        assert!(include_captured(&json!({"include_captured": true})));
        assert!(include_captured(&json!({"include_captured": "true"})));

        let schema = RecallTool.parameters_schema();
        let described = schema["properties"]["include_captured"]["description"]
            .as_str()
            .unwrap();
        assert!(described.contains("exact wording"), "{described}");
        assert_eq!(schema["properties"]["memory_id"]["type"], "string");
    }
}
