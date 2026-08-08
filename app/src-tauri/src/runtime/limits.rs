use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLimits {
    /// Maximum tool calls per single model turn
    pub max_tool_calls_per_turn: u32,
    /// Maximum total tool calls per task/session
    pub max_tool_calls_per_session: u32,
    /// Maximum time for a single model response (thinking + generation)
    pub response_timeout_seconds: u64,
    /// Maximum tokens the model can generate per response
    pub max_response_tokens: u32,
    /// Maximum retry iterations when a tool fails
    pub max_tool_retries: u32,
    /// Running counter: tool calls in current turn (reset each turn)
    #[serde(skip)]
    pub turn_tool_calls: u32,
    /// Running counter: tool calls in current session
    #[serde(skip)]
    pub session_tool_calls: u32,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_tool_calls_per_turn: 50,
            max_tool_calls_per_session: 500,
            response_timeout_seconds: 300,
            max_response_tokens: 16384,
            max_tool_retries: 3,
            turn_tool_calls: 0,
            session_tool_calls: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LimitCheckResult {
    Ok,
    TurnLimitReached,
    SessionLimitReached,
}

impl AgentLimits {
    /// Create limits appropriate for a local model connection.
    /// More conservative than cloud defaults to limit damage from tool call loops.
    pub fn for_local_model() -> Self {
        Self {
            max_tool_calls_per_turn: 10,
            max_tool_calls_per_session: 50,
            ..Default::default()
        }
    }

    /// Create limits appropriate for the given connection type.
    pub fn for_connection(config: &crate::runtime::connection::ConnectionConfig) -> Self {
        match config {
            crate::runtime::connection::ConnectionConfig::Local { .. } => Self::for_local_model(),
            _ => Self::default(),
        }
    }

    pub fn check_turn_limit(&self) -> LimitCheckResult {
        if self.turn_tool_calls >= self.max_tool_calls_per_turn {
            LimitCheckResult::TurnLimitReached
        } else {
            LimitCheckResult::Ok
        }
    }

    pub fn check_session_limit(&self) -> LimitCheckResult {
        if self.session_tool_calls >= self.max_tool_calls_per_session {
            LimitCheckResult::SessionLimitReached
        } else {
            LimitCheckResult::Ok
        }
    }

    pub fn increment_tool_calls(&mut self) {
        self.turn_tool_calls += 1;
        self.session_tool_calls += 1;
    }

    pub fn reset_turn_counter(&mut self) {
        self.turn_tool_calls = 0;
    }

    pub fn extend_session_limit(&mut self, additional: u32) {
        self.max_tool_calls_per_session += additional;
    }

    pub fn reset_session_counter(&mut self) {
        self.session_tool_calls = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limits() {
        let limits = AgentLimits::default();
        assert_eq!(limits.max_tool_calls_per_turn, 50);
        assert_eq!(limits.max_tool_calls_per_session, 500);
        assert_eq!(limits.response_timeout_seconds, 300);
        assert_eq!(limits.max_response_tokens, 16384);
        assert_eq!(limits.max_tool_retries, 3);
    }

    #[test]
    fn test_turn_limit_enforcement() {
        let mut limits = AgentLimits {
            max_tool_calls_per_turn: 3,
            ..Default::default()
        };
        assert_eq!(limits.check_turn_limit(), LimitCheckResult::Ok);
        limits.increment_tool_calls();
        limits.increment_tool_calls();
        limits.increment_tool_calls();
        assert_eq!(
            limits.check_turn_limit(),
            LimitCheckResult::TurnLimitReached
        );
        limits.reset_turn_counter();
        assert_eq!(limits.check_turn_limit(), LimitCheckResult::Ok);
        // Session counter should still be at 3
        assert_eq!(limits.session_tool_calls, 3);
    }

    #[test]
    fn test_local_model_limits() {
        let limits = AgentLimits::for_local_model();
        assert_eq!(limits.max_tool_calls_per_turn, 10);
        assert_eq!(limits.max_tool_calls_per_session, 50);
        // Other fields should be default
        assert_eq!(limits.response_timeout_seconds, 300);
        assert_eq!(limits.max_response_tokens, 16384);
    }

    #[test]
    fn test_for_connection_local() {
        let config = crate::runtime::connection::ConnectionConfig::Local {
            host: "localhost".to_string(),
            port: 11434,
            model: Some("qwen3.5".to_string()),
        };
        let limits = AgentLimits::for_connection(&config);
        assert_eq!(limits.max_tool_calls_per_turn, 10);
        assert_eq!(limits.max_tool_calls_per_session, 50);
    }

    #[test]
    fn test_for_connection_cloud() {
        let config = crate::runtime::connection::ConnectionConfig::Api {
            endpoint: "https://api.anthropic.com".to_string(),
            api_key_ref: "test".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
        };
        let limits = AgentLimits::for_connection(&config);
        assert_eq!(limits.max_tool_calls_per_turn, 50);
        assert_eq!(limits.max_tool_calls_per_session, 500);
    }

    #[test]
    fn test_session_limit_enforcement() {
        let mut limits = AgentLimits {
            max_tool_calls_per_session: 2,
            ..Default::default()
        };
        limits.increment_tool_calls();
        limits.increment_tool_calls();
        assert_eq!(
            limits.check_session_limit(),
            LimitCheckResult::SessionLimitReached
        );
        limits.extend_session_limit(5);
        assert_eq!(limits.check_session_limit(), LimitCheckResult::Ok);
    }
}
