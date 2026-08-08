use chrono::Utc;

use super::super::agent::{ConversationMessage, MessageRole};

/// Generate a rule-based compaction summary from conversation history.
/// Used as fallback when model-based summarization fails.
pub fn rule_based_summary(messages: &[ConversationMessage], working_dir: &str) -> String {
    let mut summary = String::from("## Session Summary (auto-generated)\n\n");

    // Last 5 messages
    summary.push_str("### Recent Messages\n");
    let recent = if messages.len() > 5 {
        &messages[messages.len() - 5..]
    } else {
        messages
    };
    for msg in recent {
        let role = match msg.role {
            MessageRole::System => "System",
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
            MessageRole::Tool => "Tool",
        };
        let content_preview = if msg.content.chars().count() > 200 {
            format!("{}...", msg.content.chars().take(200).collect::<String>())
        } else {
            msg.content.clone()
        };
        summary.push_str(&format!("- **{}**: {}\n", role, content_preview));
    }

    // Tool call summary
    let tool_calls: Vec<&str> = messages
        .iter()
        .filter_map(|m| m.tool_calls.as_ref())
        .flat_map(|tcs| tcs.iter().map(|tc| tc.name.as_str()))
        .collect();
    if !tool_calls.is_empty() {
        summary.push_str("\n### Tools Used\n");
        let mut tool_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for name in &tool_calls {
            *tool_counts.entry(name).or_insert(0) += 1;
        }
        for (name, count) in &tool_counts {
            summary.push_str(&format!("- {} (x{})\n", name, count));
        }
    }

    summary.push_str(&format!("\n### Working Directory\n{}\n", working_dir));

    // Message count stats
    let user_count = messages
        .iter()
        .filter(|m| m.role == MessageRole::User)
        .count();
    let assistant_count = messages
        .iter()
        .filter(|m| m.role == MessageRole::Assistant)
        .count();
    let tool_count = messages
        .iter()
        .filter(|m| m.role == MessageRole::Tool)
        .count();
    summary.push_str(&format!(
        "\n### Stats\n- {} user messages, {} assistant messages, {} tool results\n",
        user_count, assistant_count, tool_count
    ));

    summary
}

/// Build the summarization prompt for model-based compaction.
pub fn build_compaction_prompt(
    summary_context: &str,
    max_summary_tokens: u32,
) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "role": "system",
            "content": format!(
                "You are a summarization assistant. Summarize the following conversation context into a concise summary of approximately {} tokens. \
                Focus on: what the user asked for, what tools were called, what files were modified, what decisions were made, and what remains to be done. \
                Be factual and specific. Do not include greetings or filler.",
                max_summary_tokens
            ),
        }),
        serde_json::json!({
            "role": "user",
            "content": summary_context,
        }),
    ]
}

/// Build a self-summary compaction prompt.
///
/// Instead of a separate summarizer, this uses the agent's own system prompt
/// and conversation with an appended request to produce a context summary.
/// The agent has full understanding of its persona, tools, and context so the
/// summary is higher quality than an external summarizer.
pub fn build_self_summary_prompt(
    system_prompt: &str,
    conversation: &[serde_json::Value],
    max_summary_tokens: u32,
) -> Vec<serde_json::Value> {
    let mut messages = Vec::with_capacity(conversation.len() + 2);

    // Agent's own system prompt
    messages.push(serde_json::json!({
        "role": "system",
        "content": system_prompt,
    }));

    // Full conversation history (already in message format)
    messages.extend_from_slice(conversation);

    // Summary request injected as user message
    messages.push(serde_json::json!({
        "role": "user",
        "content": format!(
            "Your context window is being compacted. Produce a concise summary (~{} tokens) of this conversation for yourself. \
            Include: what was asked, what you did, what tools were called, what files were modified, \
            what decisions were made and why, and what remains to be done. \
            Be specific and factual. This summary will be your only record of the conversation so far.",
            max_summary_tokens
        ),
    }));

    messages
}

/// Build the extraction prompt for LLM pre-compaction extraction.
/// Returns a messages array for `send_provider_completion`.
pub fn build_extraction_prompt(
    conversation_context: &str,
    max_tokens: u32,
) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "role": "system",
            "content": format!(
                "You are a context extraction assistant. Analyze the following conversation and extract structured information as a JSON object. \
                Output ONLY valid JSON with no markdown fencing, no commentary, no explanation. \
                The JSON must have exactly these fields:\n\
                - \"active_task\": (string) What is the agent currently working on? What is the current state?\n\
                - \"decisions\": (array of strings) Key decisions made that matter beyond this session, with brief reasoning.\n\
                - \"files_modified\": (array of strings) File paths that were touched, each with a brief \"why\".\n\
                - \"unresolved\": (array of strings) Open questions, blockers, or things explicitly left unfinished.\n\
                - \"key_facts\": (array of strings) Durable facts learned about the user, project, or environment — not ephemeral task details.\n\
                - \"summary\": (string) A 2-3 sentence narrative summary suitable for injection as context after compaction.\n\
                - \"memory_feedback\": (array of objects, optional) For each memory that was retrieved and used in this conversation, \
                evaluate whether it was helpful (\"positive\"), misleading or ignored (\"negative\"), or neither (\"neutral\"). \
                Each object: {{\"memory_id\": \"memory:xxx\", \"signal\": \"positive\"}}. Only include non-neutral signals. \
                If no memories were retrieved, omit this field or return an empty array.\n\n\
                Be factual and specific. Do not include greetings or filler. Keep total output under {} tokens.",
                max_tokens
            ),
        }),
        serde_json::json!({
            "role": "user",
            "content": conversation_context,
        }),
    ]
}

/// Compact conversation messages by replacing old messages with a summary
/// and keeping the most recent messages intact.
///
/// If `model_summary` is provided, it's used as the summary text (from LLM-based
/// summarization). Otherwise, falls back to `rule_based_summary()`.
pub fn compact_messages(
    messages: &[ConversationMessage],
    working_dir: &str,
    keep_recent: usize,
    model_summary: Option<&str>,
) -> Vec<ConversationMessage> {
    if messages.len() <= keep_recent {
        return messages.to_vec();
    }

    let summary = match model_summary {
        Some(s) => s.to_string(),
        None => rule_based_summary(messages, working_dir),
    };

    let mut compacted = vec![ConversationMessage {
        role: MessageRole::System,
        content: format!(
            "[Context compacted — {} messages summarized]\n{}",
            messages.len().saturating_sub(keep_recent),
            summary
        ),
        content_blocks: None,
        timestamp: Utc::now(),
        token_estimate: None,
        tool_calls: None,
        tool_call_id: None,
        outcome_tag: None,
        thinking_content: None,
        responses_phase: None,
        reasoning_items: None,
    }];

    // Find safe split point — don't start in the middle of a tool_call/tool_result pair
    let start = messages.len().saturating_sub(keep_recent);
    let mut adjusted = start;
    if adjusted > 0 && adjusted < messages.len() && messages[adjusted].role == MessageRole::Tool {
        // We're in the middle of a tool result — back up to include the assistant message
        while adjusted > 0 && messages[adjusted - 1].role != MessageRole::Assistant {
            adjusted -= 1;
        }
        adjusted = adjusted.saturating_sub(1);
    }

    compacted.extend_from_slice(&messages[adjusted..]);
    compacted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::agent::ToolCallRecord;

    #[test]
    fn test_rule_based_summary() {
        let messages = vec![
            ConversationMessage {
                role: MessageRole::User,
                content: "Fix the bug in auth.rs".to_string(),
                content_blocks: None,
                timestamp: Utc::now(),
                token_estimate: None,
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            },
            ConversationMessage {
                role: MessageRole::Assistant,
                content: "I found the issue.".to_string(),
                content_blocks: None,
                timestamp: Utc::now(),
                token_estimate: None,
                tool_calls: Some(vec![ToolCallRecord {
                    id: "c1".to_string(),
                    name: "bash".to_string(),
                    arguments: serde_json::json!({}),
                }]),
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            },
        ];

        let summary = rule_based_summary(&messages, "/home/user/project");
        assert!(summary.contains("Session Summary"));
        assert!(summary.contains("bash"));
        assert!(summary.contains("/home/user/project"));
    }

    #[test]
    fn test_compaction_prompt() {
        let prompt = build_compaction_prompt("some context here", 2000);
        assert_eq!(prompt.len(), 2);
        assert_eq!(prompt[0]["role"], "system");
        assert_eq!(prompt[1]["role"], "user");
        assert_eq!(prompt[1]["content"], "some context here");
    }

    #[test]
    fn test_self_summary_prompt_structure() {
        let system = "You are a helpful assistant.";
        let conversation = vec![
            serde_json::json!({"role": "user", "content": "Hello"}),
            serde_json::json!({"role": "assistant", "content": "Hi there"}),
        ];
        let prompt = build_self_summary_prompt(system, &conversation, 1500);

        // System + 2 conversation + 1 summary request = 4
        assert_eq!(prompt.len(), 4);
        assert_eq!(prompt[0]["role"], "system");
        assert_eq!(prompt[0]["content"], system);
        assert_eq!(prompt[1]["role"], "user");
        assert_eq!(prompt[2]["role"], "assistant");
        assert_eq!(prompt[3]["role"], "user");
        let summary_request = prompt[3]["content"].as_str().unwrap();
        assert!(summary_request.contains("compacted"));
        assert!(summary_request.contains("1500"));
    }

    #[test]
    fn extraction_prompt_has_system_and_user_with_required_fields() {
        let prompt = build_extraction_prompt("User: hello\nAssistant: hi", 2000);
        assert_eq!(prompt.len(), 2);
        assert_eq!(prompt[0]["role"], "system");
        assert_eq!(prompt[1]["role"], "user");

        let system_content = prompt[0]["content"].as_str().unwrap();
        assert!(system_content.contains("active_task"));
        assert!(system_content.contains("decisions"));
        assert!(system_content.contains("files_modified"));
        assert!(system_content.contains("unresolved"));
        assert!(system_content.contains("key_facts"));
        assert!(system_content.contains("summary"));
        assert!(system_content.contains("no markdown fencing"));

        assert!(prompt[1]["content"].as_str().unwrap().contains("hello"));
    }
}
