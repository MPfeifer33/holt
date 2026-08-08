use serde_json::json;

use crate::tools::types::{Tool, ToolContext, ToolResult};

pub struct DrainInboxTool;

#[async_trait::async_trait]
impl Tool for DrainInboxTool {
    fn name(&self) -> &'static str {
        "drain_a2a_inbox"
    }

    fn description(&self) -> &'static str {
        "Fetch and clear your pending inter-agent (A2A) messages. Returns them pre-formatted. \
         Messages are delivered exactly once — they are removed from the queue when you call this. \
         Use when you are expecting a reply from another agent or checking your mail."
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
    ) -> Result<ToolResult, crate::tools::types::ToolError> {
        super::ensure_agent_a2a_enabled(context)?;

        let messages = context.a2a_registry.drain_messages(&context.agent_id).await;
        let count = messages.len() as u32;
        if count > 0 {
            tracing::info!(agent = %context.agent_id, count, "A2A inbox drained via tool");
        }

        let messages_text = if messages.is_empty() {
            serde_json::Value::Null
        } else {
            json!(crate::runtime::engine::turn_injection::format_a2a_system_section(&messages))
        };

        Ok(ToolResult {
            content: json!({
                "messages_text": messages_text,
                "count": count,
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
    use crate::runtime::a2a::A2ARegistry;
    use crate::runtime::engine::turn_injection::format_a2a_system_section;

    #[tokio::test]
    async fn drain_inbox_returns_formatted_messages_then_empties() {
        let registry = A2ARegistry::new(true); // default-enabled for test
        registry
            .send_message("agent-a", "Agent A", "agent-terminal", "spec review done")
            .await
            .unwrap();
        registry
            .send_message("agent-b", "Agent B", "agent-terminal", "ping")
            .await
            .unwrap();

        let drained = registry.drain_messages("agent-terminal").await;
        assert_eq!(drained.len(), 2);

        let text = format_a2a_system_section(&drained);
        assert!(text.contains("Direct Messages from Other Agents"));
        assert!(text.contains("Agent A") && text.contains("Agent B"));

        assert!(registry.drain_messages("agent-terminal").await.is_empty());
    }

    #[test]
    fn tool_metadata_is_well_formed() {
        let tool = DrainInboxTool;
        assert_eq!(tool.name(), "drain_a2a_inbox");
        assert!(tool.description().contains("A2A"));
        assert_eq!(
            tool.parameters_schema(),
            json!({ "type": "object", "properties": {}, "required": [] })
        );
        assert_eq!(tool.example(), Some(json!({})));
    }
}
