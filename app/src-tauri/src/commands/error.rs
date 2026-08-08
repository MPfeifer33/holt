use serde::Serialize;

/// Machine-readable error codes for Tauri command responses.
/// Serializes to snake_case strings (e.g., `NotFound` → `"not_found"`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    Validation,
    Conflict,
    Network,
    RateLimited,
    Auth,
    Internal,
    Interrupted,
}

/// Structured error returned by all Tauri commands.
/// The frontend receives `{ "code": "not_found", "message": "Agent not found: abc" }`.
#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    pub code: ErrorCode,
    pub message: String,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:?}] {}", self.code, self.message)
    }
}

impl std::error::Error for CommandError {}

// ---------------------------------------------------------------------------
// Constructors
// ---------------------------------------------------------------------------

impl CommandError {
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::NotFound,
            message: msg.into(),
        }
    }

    pub fn validation(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Validation,
            message: msg.into(),
        }
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Conflict,
            message: msg.into(),
        }
    }

    pub fn network(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Network,
            message: msg.into(),
        }
    }

    pub fn rate_limited(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::RateLimited,
            message: msg.into(),
        }
    }

    pub fn auth(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Auth,
            message: msg.into(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Internal,
            message: msg.into(),
        }
    }

    pub fn interrupted(msg: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Interrupted,
            message: msg.into(),
        }
    }
}

// Tauri IPC integration: CommandError derives Serialize, and Tauri 2 has a
// blanket `impl<T: Serialize> From<T> for InvokeError`. So CommandError
// automatically converts to InvokeError via serialization — no explicit
// From impl needed. The frontend receives { "code": "...", "message": "..." }.

// ---------------------------------------------------------------------------
// From impls: internal error types → CommandError
// ---------------------------------------------------------------------------

impl From<std::io::Error> for CommandError {
    fn from(err: std::io::Error) -> Self {
        Self::internal(err.to_string())
    }
}

impl From<serde_json::Error> for CommandError {
    fn from(err: serde_json::Error) -> Self {
        Self::internal(err.to_string())
    }
}

impl From<crate::runtime::engine::EngineError> for CommandError {
    fn from(err: crate::runtime::engine::EngineError) -> Self {
        use crate::runtime::engine::EngineError;
        match err {
            EngineError::AgentNotFound(s) => Self::not_found(s),
            EngineError::Stream(stream_err) => Self::from(stream_err),
            EngineError::ApiKeyNotFound(s) => Self::auth(s),
            EngineError::Interrupted => Self::interrupted("Agent interrupted by user"),
            EngineError::NetworkError(s) => Self::network(s),
            EngineError::AgentError(s) => Self::internal(s),
            EngineError::SessionLimitReached => Self::internal("Session limit reached"),
            EngineError::NoEndpoint => Self::internal("No model endpoint configured"),
            EngineError::ConfigError(s) => Self::internal(s),
        }
    }
}

impl From<crate::providers::types::StreamError> for CommandError {
    fn from(err: crate::providers::types::StreamError) -> Self {
        use crate::providers::types::StreamError;
        match &err {
            StreamError::Network(_) => Self::network(err.to_string()),
            StreamError::RateLimited { .. } => Self::rate_limited(err.to_string()),
            StreamError::Timeout => Self::network(err.to_string()),
            StreamError::ServerError(_)
            | StreamError::HttpError { .. }
            | StreamError::Interrupted
            | StreamError::ApiError(_) => Self::internal(err.to_string()),
        }
    }
}

impl From<crate::memory::MemoryError> for CommandError {
    fn from(err: crate::memory::MemoryError) -> Self {
        use crate::memory::MemoryError;
        match &err {
            MemoryError::NotFound(_) => Self::not_found(err.to_string()),
            MemoryError::Validation(_) => Self::internal(err.to_string()),
            MemoryError::ToolError(_) => Self::internal(err.to_string()),
            MemoryError::PinCapReached(_) | MemoryError::PinTooLarge(_) => {
                Self::internal(err.to_string())
            }
            MemoryError::Database(_) | MemoryError::Embedding(_) | MemoryError::Unavailable => {
                Self::internal(err.to_string())
            }
        }
    }
}

impl From<crate::security::keychain::KeychainError> for CommandError {
    fn from(err: crate::security::keychain::KeychainError) -> Self {
        Self::auth(err.to_string())
    }
}

impl From<crate::plugin::types::PluginError> for CommandError {
    fn from(err: crate::plugin::types::PluginError) -> Self {
        Self::internal(err.to_string())
    }
}

impl From<crate::runtime::codex::CodexExecutorError> for CommandError {
    fn from(err: crate::runtime::codex::CodexExecutorError) -> Self {
        match err {
            crate::runtime::codex::CodexExecutorError::AgentNotFound(message) => {
                Self::not_found(message)
            }
            crate::runtime::codex::CodexExecutorError::InvalidConfig
            | crate::runtime::codex::CodexExecutorError::Spawn(_)
            | crate::runtime::codex::CodexExecutorError::Execution(_)
            | crate::runtime::codex::CodexExecutorError::Io(_)
            | crate::runtime::codex::CodexExecutorError::Json(_)
            | crate::runtime::codex::CodexExecutorError::ServerSpawn(_)
            | crate::runtime::codex::CodexExecutorError::ServerConnection(_)
            | crate::runtime::codex::CodexExecutorError::ServerDied(_)
            | crate::runtime::codex::CodexExecutorError::ThreadForkFailed(_) => {
                Self::internal(err.to_string())
            }
            crate::runtime::codex::CodexExecutorError::Interrupted => {
                Self::interrupted(err.to_string())
            }
            crate::runtime::codex::CodexExecutorError::Network(_) => Self::network(err.to_string()),
            crate::runtime::codex::CodexExecutorError::RateLimited { .. }
            | crate::runtime::codex::CodexExecutorError::CreditsExhausted => {
                Self::rate_limited(err.to_string())
            }
        }
    }
}
