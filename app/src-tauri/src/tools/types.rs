use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::RwLock;

use crate::config::app_config::AppConfig;
use crate::runtime::a2a::A2ARegistry;
use crate::runtime::agent::AgentSlot;
use crate::runtime::subagent::SubagentManager;
use crate::trace::engine::TraceEngine;
use crate::AppState;

use super::shell::ShellManager;
use super::vfl::VirtualFileLockRegistry;

/// Context passed to every tool execution, providing access to agent state and shared resources.
#[derive(Clone)]
pub struct ToolContext {
    pub agent_id: String,
    pub working_directory: PathBuf,
    pub workspace_root: PathBuf,
    pub trace_engine: Arc<TraceEngine>,
    pub app_handle: Option<AppHandle>,
    pub config: Arc<RwLock<AppConfig>>,
    pub shell_manager: Arc<RwLock<ShellManager>>,
    pub session_id: String,
    pub vfl_registry: Arc<VirtualFileLockRegistry>,
    pub subagent_manager: Arc<SubagentManager>,
    pub agents: Arc<RwLock<Vec<AgentSlot>>>,
    pub a2a_registry: Arc<A2ARegistry>,
    /// Loop-local guard for A2A sends. When present, tools can suppress
    /// repeated sends to the same target during a single agentic loop session.
    pub a2a_send_guard: Option<Arc<RwLock<HashSet<String>>>>,
    /// Access to full AppState for subagent spawning (needs keychain, rate_limiter, etc.)
    pub app_state: Option<Arc<AppState>>,
    /// Shared HTTP client — reuse instead of creating per-invocation clients.
    /// Populated from `AppState.http_client` at construction.
    pub http_client: reqwest::Client,
}

/// Successful result from a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: serde_json::Value,
    pub truncated: bool,
    pub trace_id: Option<String>,
    /// Image content blocks returned by tools (e.g., read_file on images, screenshot_url).
    /// When present, these are propagated to ConversationMessage.content_blocks.
    #[serde(default)]
    pub image_content: Option<Vec<crate::runtime::agent::ContentBlock>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolAuthorityClass {
    #[default]
    Informational,
    Effectful,
    EffectfulVerified,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultOutcome {
    #[default]
    Success,
    Partial,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolVerificationStatus {
    #[default]
    NotRequired,
    Verified,
    VerificationFailed,
    VerificationSkipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionReceipt {
    pub authority_class: ToolAuthorityClass,
    pub executed: bool,
    pub execution_status: String,
    pub verified: bool,
    pub verification_status: ToolVerificationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result_trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Structured error from a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolError {
    pub code: ToolErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for ToolError {}

impl From<std::io::Error> for ToolError {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::NotFound => Self {
                code: ToolErrorCode::FileNotFound,
                message: err.to_string(),
                retryable: false,
            },
            std::io::ErrorKind::PermissionDenied => Self {
                code: ToolErrorCode::PermissionDenied,
                message: err.to_string(),
                retryable: false,
            },
            std::io::ErrorKind::TimedOut => Self {
                code: ToolErrorCode::CommandTimeout,
                message: err.to_string(),
                retryable: true,
            },
            _ => Self {
                code: ToolErrorCode::InternalError,
                message: err.to_string(),
                retryable: false,
            },
        }
    }
}

impl From<serde_json::Error> for ToolError {
    fn from(err: serde_json::Error) -> Self {
        Self {
            code: ToolErrorCode::InvalidInput,
            message: err.to_string(),
            retryable: false,
        }
    }
}

/// Error codes for all tool failures (PRD Section 5.9.8).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ToolErrorCode {
    PathOutOfBounds,
    FileNotFound,
    PermissionDenied,
    HilDenied,
    VflTimeout,
    ProcessNotFound,
    ProcessLimitReached,
    SubagentLimitReached,
    A2ANotEnabled,
    CommandTimeout,
    NetworkError,
    InvalidInput,
    SearchProviderNotConfigured,
    RateLimited,
    InternalError,
}

/// Engine-level control flow semantics for successful tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSuccessBehavior {
    ContinueLoop,
    EndTurnAfterSuccess,
}

/// The core trait that all tools implement.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn authority_class(&self) -> ToolAuthorityClass {
        ToolAuthorityClass::Informational
    }
    fn success_behavior(&self) -> ToolSuccessBehavior {
        ToolSuccessBehavior::ContinueLoop
    }
    /// Optional usage example as JSON arguments. Used by list_available_tools
    /// to show agents how to call this tool.
    fn example(&self) -> Option<serde_json::Value> {
        None
    }
    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError>;
}
