use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Trace types that determine truncation limits (PRD Section 6.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TraceType {
    HilMessage,
    AgentResponse,
    ToolCall,
    ToolResult,
    MemoryPipeline,
    SessionMemoryLifecycle,
    SubagentSpawn,
    SubagentResult,
    Error,
    ConfigChange,
    PluginEvent,
    A2AMessage,
}

impl TraceType {
    /// Returns the string representation for storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HilMessage => "hil_message",
            Self::AgentResponse => "agent_response",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::MemoryPipeline => "memory_pipeline",
            Self::SessionMemoryLifecycle => "session_memory_lifecycle",
            Self::SubagentSpawn => "subagent_spawn",
            Self::SubagentResult => "subagent_result",
            Self::Error => "error",
            Self::ConfigChange => "config_change",
            Self::PluginEvent => "plugin_event",
            Self::A2AMessage => "a2a_message",
        }
    }

    /// Maximum content size in characters for this trace type (PRD Section 6.3).
    pub fn max_chars(&self) -> usize {
        match self {
            Self::Error => 5000,
            Self::MemoryPipeline | Self::SessionMemoryLifecycle => 8000,
            Self::HilMessage
            | Self::AgentResponse
            | Self::ToolResult
            | Self::SubagentResult
            | Self::ConfigChange
            | Self::A2AMessage => 2000,
            Self::ToolCall | Self::PluginEvent => 1000,
            Self::SubagentSpawn => 500,
        }
    }
}

impl FromStr for TraceType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "hil_message" => Ok(Self::HilMessage),
            "agent_response" => Ok(Self::AgentResponse),
            "tool_call" => Ok(Self::ToolCall),
            "tool_result" => Ok(Self::ToolResult),
            "memory_pipeline" => Ok(Self::MemoryPipeline),
            "session_memory_lifecycle" => Ok(Self::SessionMemoryLifecycle),
            "subagent_spawn" => Ok(Self::SubagentSpawn),
            "subagent_result" => Ok(Self::SubagentResult),
            "error" => Ok(Self::Error),
            "config_change" => Ok(Self::ConfigChange),
            "plugin_event" => Ok(Self::PluginEvent),
            "a2a_message" => Ok(Self::A2AMessage),
            other => Err(format!("Unknown trace type: '{}'", other)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TraceOutcome {
    Success,
    Failure { reason: String },
    Partial { details: String },
    Unknown,
}

/// UTF-8 safe truncation using char_indices() per PRD Section 6.3.
/// Uses head+tail strategy: keep first half chars + last half chars with
/// a truncation notice in between.
pub fn truncate_trace_content(trace_type: &TraceType, content: &str) -> String {
    let max = trace_type.max_chars();
    let char_count = content.chars().count();

    if char_count <= max {
        return content.to_string();
    }

    let half = max / 2;

    // Find byte offset for end of first `half` chars
    let start_end = content
        .char_indices()
        .nth(half)
        .map(|(i, _)| i)
        .unwrap_or(content.len());

    // Find byte offset for start of last `half` chars
    let tail_start = content
        .char_indices()
        .rev()
        .nth(half - 1)
        .map(|(i, _)| i)
        .unwrap_or(0);

    format!(
        "{}...[TRUNCATED: {} chars total]...{}",
        &content[..start_end],
        char_count,
        &content[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_truncation_when_under_limit() {
        let content = "Short message";
        let result = truncate_trace_content(&TraceType::HilMessage, content);
        assert_eq!(result, content);
    }

    #[test]
    fn test_truncation_at_limit() {
        // HilMessage limit is 2000 chars
        let content = "a".repeat(2000);
        let result = truncate_trace_content(&TraceType::HilMessage, &content);
        assert_eq!(result, content); // Exactly at limit, no truncation
    }

    #[test]
    fn test_truncation_over_limit() {
        let content = "x".repeat(3000);
        let result = truncate_trace_content(&TraceType::HilMessage, &content);
        assert!(result.contains("TRUNCATED: 3000 chars total"));
        assert!(result.len() < content.len());
    }

    #[test]
    fn test_truncation_preserves_utf8() {
        // CJK characters (3 bytes each in UTF-8)
        let content = "漢".repeat(3000);
        let result = truncate_trace_content(&TraceType::HilMessage, &content);
        assert!(result.contains("TRUNCATED"));
        // Verify the result is valid UTF-8 (would panic if not)
        let _ = result.chars().count();
    }

    #[test]
    fn test_truncation_with_emoji() {
        // Emoji (4 bytes each in UTF-8)
        let content = "🎉".repeat(3000);
        let result = truncate_trace_content(&TraceType::HilMessage, &content);
        assert!(result.contains("TRUNCATED"));
        let _ = result.chars().count(); // Verify valid UTF-8
    }

    #[test]
    fn test_error_gets_more_room() {
        let content = "e".repeat(4000);
        // Error limit is 5000, so 4000 chars should NOT be truncated
        let result = truncate_trace_content(&TraceType::Error, &content);
        assert_eq!(result, content);
    }

    #[test]
    fn test_subagent_spawn_tight_limit() {
        let content = "s".repeat(600);
        // SubagentSpawn limit is 500 chars
        let result = truncate_trace_content(&TraceType::SubagentSpawn, &content);
        assert!(result.contains("TRUNCATED: 600 chars total"));
    }

    #[test]
    fn test_trace_type_round_trip() {
        for tt in [
            TraceType::HilMessage,
            TraceType::AgentResponse,
            TraceType::ToolCall,
            TraceType::ToolResult,
            TraceType::MemoryPipeline,
            TraceType::SessionMemoryLifecycle,
            TraceType::SubagentSpawn,
            TraceType::SubagentResult,
            TraceType::Error,
            TraceType::ConfigChange,
            TraceType::PluginEvent,
            TraceType::A2AMessage,
        ] {
            let s = tt.as_str();
            let parsed: TraceType = s.parse().unwrap();
            assert_eq!(tt, parsed);
        }
    }
}
