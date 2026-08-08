use serde_json::json;

use crate::runtime::agent::MessageRole;
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct ContextStatusTool;

#[async_trait::async_trait]
impl Tool for ContextStatusTool {
    fn name(&self) -> &str {
        "context_status"
    }

    fn description(&self) -> &str {
        "Check how much of your context window is used and how much remains. Use this to decide whether to save important state to memory before compaction approaches."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(
        &self,
        _arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let agents = context.agents.read().await;
        let agent = agents
            .iter()
            .find(|a| a.id == context.agent_id)
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InternalError,
                message: "Agent not found".to_string(),
                retryable: false,
            })?;

        let context_tokens = agent.current_context_tokens;
        let context_window = agent.context_window_size;
        let session_prompt = agent.session_prompt_tokens;
        let session_completion = agent.session_completion_tokens;
        let total_api_calls = agent.metadata.total_api_calls;

        // Count messages by role
        let mut user_count = 0u32;
        let mut assistant_count = 0u32;
        let mut tool_count = 0u32;
        let mut system_count = 0u32;
        for msg in &agent.conversation.messages {
            match msg.role {
                MessageRole::User => user_count += 1,
                MessageRole::Assistant => assistant_count += 1,
                MessageRole::Tool => tool_count += 1,
                MessageRole::System => system_count += 1,
            }
        }

        drop(agents);

        // Build response with conditional field omission
        let mut result = serde_json::Map::new();
        result.insert("source".to_string(), json!("api_reported"));
        result.insert("context_window_tokens".to_string(), json!(context_window));

        // Token fields — only when we have real API data
        if context_tokens > 0 && context_window > 0 {
            let remaining = context_window.saturating_sub(context_tokens as u32);
            let percent_used = (context_tokens as f64 / context_window as f64 * 100.0) as u32;
            result.insert("context_tokens".to_string(), json!(context_tokens));
            result.insert("remaining_tokens".to_string(), json!(remaining));
            result.insert("percent_used".to_string(), json!(percent_used));
            result.insert(
                "compaction_approaching".to_string(),
                json!(percent_used >= 70),
            );
            result.insert("compaction_imminent".to_string(), json!(percent_used >= 85));
            result.insert(
                "status".to_string(),
                json!(if percent_used >= 85 {
                    "CRITICAL"
                } else if percent_used >= 70 {
                    "WARNING"
                } else {
                    "OK"
                }),
            );
        } else {
            result.insert("status".to_string(), json!("OK"));
        }

        result.insert("num_turns".to_string(), json!(user_count + assistant_count));
        result.insert(
            "messages".to_string(),
            json!({
                "user": user_count,
                "assistant": assistant_count,
                "tool": tool_count,
                "system": system_count,
                "total": user_count + assistant_count + tool_count + system_count,
            }),
        );
        result.insert(
            "session_tokens".to_string(),
            json!({
                "prompt": session_prompt,
                "completion": session_completion,
                "total_api_calls": total_api_calls,
            }),
        );

        // Session cost — only when cost exists
        if let Some(ref app_state) = context.app_state {
            let cost = app_state
                .trace_engine
                .query_session_cost(&context.agent_id, &context.session_id)
                .unwrap_or(0);
            if cost > 0 {
                result.insert("session_cost_usd_cents".to_string(), json!(cost));
            }

            // Compaction pressure from memory bridge
            if let Some(memory_engine) = app_state.get_memory_engine() {
                match memory_engine
                    .compaction_pressure(&context.agent_id, &context.session_id)
                    .await
                {
                    Ok(pressure) => {
                        result.insert("compaction_pressure".to_string(), pressure);
                    }
                    Err(e) => {
                        tracing::debug!(
                            agent_id = context.agent_id,
                            "Failed to fetch compaction pressure: {e}"
                        );
                    }
                }
            }
        }

        Ok(ToolResult {
            content: serde_json::Value::Object(result),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    fn build_context_json(context_tokens: u64, context_window: u32) -> serde_json::Value {
        let mut result = serde_json::Map::new();
        result.insert("source".to_string(), json!("api_reported"));
        result.insert("context_window_tokens".to_string(), json!(context_window));

        if context_tokens > 0 && context_window > 0 {
            let remaining = context_window.saturating_sub(context_tokens as u32);
            let percent = (context_tokens as f64 / context_window as f64 * 100.0) as u32;
            result.insert("context_tokens".to_string(), json!(context_tokens));
            result.insert("remaining_tokens".to_string(), json!(remaining));
            result.insert("percent_used".to_string(), json!(percent));
            result.insert("compaction_approaching".to_string(), json!(percent >= 70));
            result.insert("compaction_imminent".to_string(), json!(percent >= 85));
        }

        serde_json::Value::Object(result)
    }

    #[test]
    fn omits_token_fields_when_no_api_data() {
        let json = build_context_json(0, 125_000);
        assert!(json.get("context_tokens").is_none());
        assert!(json.get("remaining_tokens").is_none());
        assert!(json.get("percent_used").is_none());
        assert!(json.get("compaction_approaching").is_none());
        assert!(json.get("compaction_imminent").is_none());
        assert!(json.get("source").is_some());
        assert!(json.get("context_window_tokens").is_some());
    }

    #[test]
    fn includes_token_fields_when_api_data_present() {
        let json = build_context_json(85_000, 125_000);
        assert_eq!(json["context_tokens"], 85_000);
        assert_eq!(json["remaining_tokens"], 40_000);
        assert_eq!(json["percent_used"], 68);
        assert_eq!(json["compaction_approaching"], false);
        assert_eq!(json["compaction_imminent"], false);
    }

    #[test]
    fn compaction_approaching_at_70_percent() {
        let json = build_context_json(87_500, 125_000);
        assert_eq!(json["compaction_approaching"], true);
        assert_eq!(json["compaction_imminent"], false);
    }

    #[test]
    fn compaction_imminent_at_85_percent() {
        let json = build_context_json(106_250, 125_000);
        assert_eq!(json["compaction_approaching"], true);
        assert_eq!(json["compaction_imminent"], true);
    }
}
