//! Shared types for the provider abstraction layer.
//!
//! Contains the `CompletionProvider` trait, streaming event types,
//! assembled response type, and error types used across all providers.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::runtime::agent::ToolCallRecord;

// =============================================================================
// Streaming event types
// =============================================================================

/// Event emitted to the frontend during streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    pub agent_id: String,
    pub event_type: StreamEventType,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamEventType {
    Token,
    ToolCall,
    ToolResult,
    Done,
    Error,
    A2AMessage,
    ContextWarning,
    ConversationUpdated,
    Thinking,
    SandboxStatus,
    ForkCompleted,
    SessionMemoryUpdated,
    SessionMemoryWarning,
    SessionMemoryError,
    RateLimitUpdate,
    GoalUpdate,
}

/// A fully assembled response from the model (after all SSE chunks are combined).
#[derive(Debug, Clone)]
pub struct AssembledResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCallRecord>,
    pub usage: Option<UsageInfo>,
    pub finish_reason: Option<String>,
    pub thinking_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageInfo {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}

// =============================================================================
// Error types
// =============================================================================

/// Errors that can occur during streaming.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("Network error: {0}")]
    Network(reqwest::Error),
    #[error("Rate limited (429)")]
    RateLimited { retry_after: Option<u64> },
    #[error("Server error: {0}")]
    ServerError(u16),
    #[error("HTTP error {status}: {body}")]
    HttpError { status: u16, body: String },
    #[error("Response timeout")]
    Timeout,
    #[error("Stream interrupted")]
    Interrupted,
    #[error("API error: {0}")]
    ApiError(String),
}

/// Errors that can occur during provider construction.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("invalid connection config: {0}")]
    InvalidConfig(String),
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("unsupported connection type for protocol: {0}")]
    UnsupportedConnection(String),
}

// =============================================================================
// CompletionProvider trait
// =============================================================================

/// Trait for HTTP-based completion providers.
///
/// Each provider (OpenAI, Gemini, xAI, Anthropic) implements this trait.
/// The engine calls providers through this uniform interface.
///
/// The `client` is passed in (not owned) so the engine can manage connection
/// pooling and keep-alive across requests.
#[async_trait]
pub trait CompletionProvider: Send + Sync + std::fmt::Debug + 'static {
    /// Human-readable provider name for logging/tracing.
    fn name(&self) -> &'static str;

    /// Send a streaming completion request.
    ///
    /// Calls `on_token` for each streaming event (tokens, thinking, tool calls, done).
    /// Returns the fully assembled response after the stream completes.
    async fn send_completion(
        &self,
        client: &reqwest::Client,
        messages: &[serde_json::Value],
        tools: Option<&[serde_json::Value]>,
        max_tokens: u32,
        on_token: &(dyn Fn(StreamEvent) + Send + Sync),
        agent_id: &str,
    ) -> Result<AssembledResponse, StreamError>;
}
