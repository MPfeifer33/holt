//! Codex V2 protocol types for app-server integration.
//!
//! All types derive `Serialize, Deserialize` with `camelCase` field naming to match
//! the Codex V2 JSON schema convention. Unknown fields are silently ignored for forward
//! compatibility — Codex may add new fields in future versions.
//!
//! Schema source: `codex app-server generate-json-schema --experimental` (v0.142.5, 288 files)

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================================================================
// JSON-RPC envelope types
// ============================================================================

/// Outgoing JSON-RPC 2.0 request.
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest<T: Serialize> {
    pub jsonrpc: &'static str,
    pub method: String,
    pub params: T,
    pub id: u64,
}

impl<T: Serialize> JsonRpcRequest<T> {
    pub fn new(method: impl Into<String>, params: T, id: u64) -> Self {
        Self {
            jsonrpc: "2.0",
            method: method.into(),
            params,
            id,
        }
    }
}

/// Incoming JSON-RPC 2.0 response.
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
    pub id: Option<u64>,
}

/// JSON-RPC error object.
#[derive(Debug, Deserialize, Clone)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON-RPC error {}: {}", self.code, self.message)
    }
}

/// Incoming JSON-RPC 2.0 notification (no `id` field).
#[derive(Debug, Deserialize)]
pub struct JsonRpcNotification {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<Value>,
}

// ============================================================================
// Request params — Thread lifecycle
// ============================================================================

/// Parameters for `thread/start`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality: Option<Personality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
}

/// Parameters for `thread/resume`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxMode>,
    /// When true, omit turn history from the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_turns: Option<bool>,
}

/// Parameters for `thread/fork`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<bool>,
    /// When true, omit turn history from the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_turns: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
}

// ============================================================================
// Request params — Turn lifecycle
// ============================================================================

/// Parameters for `turn/start`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    pub thread_id: String,
    pub input: Vec<UserInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality: Option<Personality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ReasoningSummary>,
}

/// User input for a turn — text, images, or local images.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UserInput {
    /// Plain text input.
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(rename = "text_elements")]
        text_elements: Vec<Value>,
    },
    /// Image via URL (including data: base64 URIs).
    #[serde(rename = "image")]
    Image {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },
    /// Image via local file path.
    #[serde(rename = "localImage")]
    LocalImage {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },
}

/// Parameters for `turn/interrupt`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    pub thread_id: String,
    pub turn_id: String,
}

// ============================================================================
// Request params — Thread operations
// ============================================================================

/// Parameters for `thread/compact/start`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadCompactStartParams {
    pub thread_id: String,
}

/// Parameters for `thread/rollback`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRollbackParams {
    pub thread_id: String,
    /// Number of turns to drop from the end. Must be >= 1.
    /// Only modifies thread history — does NOT revert local file changes.
    pub num_turns: u32,
}

/// Parameters for `thread/settings/update`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSettingsUpdateParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality: Option<Personality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ReasoningSummary>,
}

/// Parameters for `thread/goal/set`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalSetParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ThreadGoalStatus>,
}

/// Parameters for `thread/goal/get`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalGetParams {
    pub thread_id: String,
}

/// Parameters for `thread/goal/clear`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalClearParams {
    pub thread_id: String,
}

/// Parameters for `thread/inject_items`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadInjectItemsParams {
    pub thread_id: String,
    /// Raw Responses API items to append to the thread's model-visible history.
    pub items: Vec<Value>,
}

/// Parameters for `thread/memoryMode/set`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadMemoryModeSetParams {
    pub thread_id: String,
    pub mode: ThreadMemoryMode,
}

/// Parameters for `thread/name/set`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSetNameParams {
    pub thread_id: String,
    pub name: String,
}

/// Parameters for `thread/shellCommand`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadShellCommandParams {
    pub thread_id: String,
    /// Shell command string. Runs unsandboxed with full access.
    pub command: String,
}

/// Parameters for `thread/search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSearchParams {
    pub search_term: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_direction: Option<SortDirection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_key: Option<ThreadSortKey>,
}

/// Parameters for `thread/delete`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDeleteParams {
    pub thread_id: String,
}

/// Parameters for `thread/approveGuardianDeniedAction`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadApproveActionParams {
    pub thread_id: String,
    /// Serialized `GuardianAssessmentEvent`.
    pub event: Value,
}

/// Parameters for `mcpServerStatus/list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMcpServerStatusParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Parameters for `thread/backgroundTerminals/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadBgTerminalsListParams {
    pub thread_id: String,
}

/// Parameters for `thread/backgroundTerminals/terminate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadBgTerminalsTerminateParams {
    pub thread_id: String,
}

/// Parameters for `thread/backgroundTerminals/clean`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadBgTerminalsCleanParams {
    pub thread_id: String,
}

// ============================================================================
// Response types
// ============================================================================

/// Common response for thread/start, thread/fork, thread/resume.
/// All three return the same shape: thread metadata + settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResponse {
    pub thread: Thread,
    pub model: String,
    pub model_provider: String,
    pub cwd: String,
    pub approval_policy: Value, // Preserve as Value — complex enum
    pub approvals_reviewer: ApprovalsReviewer,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
}

/// Thread metadata.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    pub session_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub cwd: String,
    pub model_provider: String,
    pub preview: String,
    pub status: ThreadStatus,
    pub ephemeral: bool,
    pub cli_version: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub forked_from_id: Option<String>,
    #[serde(default)]
    pub parent_thread_id: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    /// Turns are only populated on resume/fork/read, not always.
    #[serde(default)]
    pub turns: Option<Vec<Value>>,
}

/// Response for `turn/start`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartResponse {
    pub turn: Value, // Turn object — complex, preserve as Value
}

/// Response for `thread/goal/get`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalGetResponse {
    pub goal: Option<ThreadGoal>,
}

/// Response for `thread/search`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSearchResponse {
    pub data: Vec<Value>, // ThreadSearchResult items
    pub next_cursor: Option<String>,
    #[serde(default)]
    pub backwards_cursor: Option<String>,
}

/// Response for `mcpServerStatus/list`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMcpServerStatusResponse {
    pub data: Vec<Value>, // McpServerStatus items
    pub next_cursor: Option<String>,
}

// ============================================================================
// Notification types (events we receive from the app-server)
// ============================================================================

/// Thread started — capture the thread ID.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartedNotification {
    pub thread: Thread,
}

/// Turn started — capture the turn ID for cancellation.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartedNotification {
    pub thread_id: String,
    pub turn: Value, // Turn object
}

/// Streaming text delta from the agent.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageDeltaNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
}

/// Turn plan update (reasoning/thinking content).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPlanUpdatedNotification {
    pub thread_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(flatten)]
    pub extra: Value,
}

/// Turn completed — contains the full Turn with all items.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCompletedNotification {
    pub thread_id: String,
    pub turn: Value, // Full Turn object
}

/// Token usage update — real counts per turn + cumulative.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTokenUsageUpdatedNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub token_usage: ThreadTokenUsage,
}

/// Token usage data.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTokenUsage {
    pub last: TokenUsageBreakdown,
    pub total: TokenUsageBreakdown,
    #[serde(default)]
    pub model_context_window: Option<i64>,
}

/// Token count breakdown.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageBreakdown {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

/// Goal status update.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalUpdatedNotification {
    pub thread_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    pub goal: ThreadGoal,
}

/// Thread goal data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoal {
    pub thread_id: String,
    pub objective: String,
    pub status: ThreadGoalStatus,
    pub tokens_used: i64,
    #[serde(default)]
    pub token_budget: Option<i64>,
    pub time_used_seconds: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Rate limit update from the account.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRateLimitsUpdatedNotification {
    pub rate_limits: RateLimitSnapshot,
}

/// Rate limit snapshot.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitSnapshot {
    #[serde(default)]
    pub primary: Option<RateLimitWindow>,
    #[serde(default)]
    pub secondary: Option<RateLimitWindow>,
    #[serde(default)]
    pub credits: Option<CreditsSnapshot>,
    #[serde(default)]
    pub individual_limit: Option<SpendControlLimitSnapshot>,
    #[serde(default)]
    pub rate_limit_reached_type: Option<RateLimitReachedType>,
    #[serde(default)]
    pub plan_type: Option<PlanType>,
}

/// Rate limit window (primary or secondary).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitWindow {
    pub used_percent: i32,
    #[serde(default)]
    pub resets_at: Option<i64>,
    #[serde(default)]
    pub window_duration_mins: Option<i64>,
}

/// Credit balance snapshot.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditsSnapshot {
    pub has_credits: bool,
    #[serde(default)]
    pub balance: Option<String>,
    pub unlimited: bool,
}

/// Spend control limit snapshot.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendControlLimitSnapshot {
    pub limit: String,
    pub used: String,
    pub remaining_percent: i32,
    pub resets_at: i64,
}

/// Error notification from the app-server.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub error: TurnError,
    pub will_retry: bool,
}

/// Turn error details.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnError {
    pub message: String,
    #[serde(default)]
    pub additional_details: Option<String>,
    #[serde(default)]
    pub codex_error_info: Option<CodexErrorInfo>,
}

/// Guardian approval review started — agent is requesting approval for an action.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalReviewStartedNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub review_id: String,
    pub action: Value, // GuardianApprovalReviewAction — complex union, preserve as Value
    pub review: GuardianApprovalReview,
    pub started_at_ms: i64,
    #[serde(default)]
    pub target_item_id: Option<String>,
}

/// Guardian approval review state.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardianApprovalReview {
    pub status: GuardianApprovalReviewStatus,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub risk_level: Option<GuardianRiskLevel>,
}

/// MCP server status update.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatusUpdatedNotification {
    pub name: String,
    pub status: McpServerStartupState,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
}

/// Context compacted (deprecated — use ContextCompaction item type).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactedNotification {
    pub thread_id: String,
    pub turn_id: String,
}

/// Turn diff update (file changes).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnDiffUpdatedNotification {
    pub thread_id: String,
    #[serde(flatten)]
    pub extra: Value,
}

/// Thread status change.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStatusChangedNotification {
    pub thread_id: String,
    #[serde(flatten)]
    pub extra: Value,
}

/// `item/started` / `item/completed` notification params.
///
/// Carries the full thread item. The agentMessage `item/completed` is the
/// authoritative source of the final reply text (turn/completed ships an
/// empty items list), and tool-call items carry the real tool names.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemNotification {
    pub thread_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    pub item: ThreadItem,
}

/// A thread item, tagged by `type` on the wire. Only the variants Holt
/// acts on are modeled; everything else (reasoning, plan, fileChange, …)
/// lands in `Other`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadItem {
    #[serde(rename_all = "camelCase")]
    AgentMessage { id: String, text: String },
    #[serde(rename_all = "camelCase")]
    McpToolCall {
        id: String,
        tool: String,
        #[serde(default)]
        server: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        arguments: Option<Value>,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        error: Option<Value>,
    },
    #[serde(rename_all = "camelCase")]
    DynamicToolCall {
        id: String,
        tool: String,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        arguments: Option<Value>,
    },
    #[serde(rename_all = "camelCase")]
    CommandExecution {
        id: String,
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        exit_code: Option<i64>,
        #[serde(default)]
        aggregated_output: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    WebSearch {
        id: String,
        #[serde(default)]
        query: Option<String>,
    },
    /// Forward-compat: item types we don't act on.
    #[serde(other)]
    Other,
}

/// Command execution output delta.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutputDeltaNotification {
    pub thread_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(flatten)]
    pub extra: Value,
}

/// MCP tool call progress.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallProgressNotification {
    pub thread_id: String,
    #[serde(flatten)]
    pub extra: Value,
}

/// Thread name updated.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadNameUpdatedNotification {
    pub thread_id: String,
    #[serde(flatten)]
    pub extra: Value,
}

// ============================================================================
// Enums — Codex V2 protocol values
// ============================================================================

/// Approval policy for a thread or turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalPolicy {
    Untrusted,
    OnFailure,
    OnRequest,
    Never,
}

// Note: The "granular" variant is a separate struct shape in the V2 protocol.
// For our use case, we only need the simple enum values (primarily "never").
// If granular approval is needed, construct it as a raw Value.

/// Who reviews approval requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalsReviewer {
    User,
    AutoReview,
    GuardianSubagent,
}

/// Agent personality preset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Personality {
    None,
    Friendly,
    Pragmatic,
}

/// Sandbox mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

/// The lane-wide Codex trust posture: full access, never ask.
///
/// Sent on EVERY thread entry path (start/resume/fork) so no thread —
/// including ones persisted before this policy existed — reverts to a
/// restrictive sandbox. `Never` alone is not enough: without the explicit
/// sandbox mode, Codex is told "don't ask" while still walled in, and every
/// blocked action becomes a silent stall. Nothing in this lane is
/// auto-declined; see `build_server_request_reply` for the backstop.
pub const FULL_TRUST_APPROVAL_POLICY: ApprovalPolicy = ApprovalPolicy::Never;
pub const FULL_TRUST_SANDBOX: SandboxMode = SandboxMode::DangerFullAccess;

/// Image detail level.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ImageDetail {
    Auto,
    Low,
    High,
    Original,
}

/// Reasoning summary mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningSummary {
    Auto,
    Concise,
    Detailed,
    None,
}

/// Thread goal status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ThreadGoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}

impl std::fmt::Display for ThreadGoalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Paused => write!(f, "paused"),
            Self::Blocked => write!(f, "blocked"),
            Self::UsageLimited => write!(f, "usage limited"),
            Self::BudgetLimited => write!(f, "budget limited"),
            Self::Complete => write!(f, "complete"),
        }
    }
}

/// Thread runtime status.
///
/// Internally tagged on the wire: `{"type": "idle"}`, `{"type": "active",
/// "activeFlags": ["waitingOnApproval"]}`, … (per the generated protocol
/// schema for codex-cli 0.142.5).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadStatus {
    NotLoaded,
    Idle,
    SystemError,
    Active {
        #[serde(rename = "activeFlags", default)]
        active_flags: Vec<String>,
    },
    /// Forward-compat: variants added by newer codex versions.
    #[serde(other)]
    Unknown,
}

/// Thread memory mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ThreadMemoryMode {
    Enabled,
    Disabled,
}

/// MCP server startup state.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum McpServerStartupState {
    Starting,
    Ready,
    Failed,
    Cancelled,
}

/// Rate limit reached type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitReachedType {
    RateLimitReached,
    WorkspaceOwnerCreditsDepleted,
    WorkspaceMemberCreditsDepleted,
    WorkspaceOwnerUsageLimitReached,
    WorkspaceMemberUsageLimitReached,
    #[serde(other)]
    Unknown,
}

/// Account plan type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PlanType {
    Free,
    Go,
    Plus,
    Pro,
    #[serde(rename = "prolite")]
    ProLite,
    Team,
    #[serde(rename = "self_serve_business_usage_based")]
    SelfServeBusinessUsageBased,
    Business,
    #[serde(rename = "enterprise_cbp_usage_based")]
    EnterpriseCbpUsageBased,
    Enterprise,
    Edu,
    #[serde(other)]
    Unknown,
}

/// Codex error info — classified error from the upstream API.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum CodexErrorInfo {
    ContextWindowExceeded,
    UsageLimitExceeded,
    ServerOverloaded,
    CyberPolicy,
    InternalServerError,
    Unauthorized,
    BadRequest,
    ThreadRollbackFailed,
    SandboxError,
    Other,
    // Complex variants with HTTP status codes — deserialize as untagged
    #[serde(untagged)]
    HttpConnectionFailed {
        http_status_code: Option<u16>,
    },
}

/// Guardian approval review status.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum GuardianApprovalReviewStatus {
    InProgress,
    Approved,
    Denied,
    TimedOut,
    Aborted,
}

/// Guardian risk level.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GuardianRiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// Sort direction for thread search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    Asc,
    Desc,
}

/// Sort key for thread search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadSortKey {
    CreatedAt,
    UpdatedAt,
    RecencyAt,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_rpc_request_serializes() {
        let req = JsonRpcRequest::new(
            "thread/start",
            ThreadStartParams {
                model: Some("gpt-5.5".into()),
                ..Default::default()
            },
            1,
        );
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["method"], "thread/start");
        assert_eq!(json["id"], 1);
        assert_eq!(json["params"]["model"], "gpt-5.5");
        // Optional fields should not be present
        assert!(json["params"]["cwd"].is_null() || json["params"].get("cwd").is_none());
    }

    #[test]
    fn json_rpc_response_deserializes() {
        let json = json!({
            "jsonrpc": "2.0",
            "result": {"thread": {"id": "t_123"}},
            "id": 1
        });
        let resp: JsonRpcResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn json_rpc_error_response_deserializes() {
        let json = json!({
            "jsonrpc": "2.0",
            "error": {"code": -32600, "message": "Invalid Request"},
            "id": 2
        });
        let resp: JsonRpcResponse = serde_json::from_value(json).unwrap();
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Invalid Request");
    }

    #[test]
    fn json_rpc_notification_deserializes() {
        let json = json!({
            "jsonrpc": "2.0",
            "method": "agent/message/delta",
            "params": {"threadId": "t_1", "turnId": "turn_1", "itemId": "item_1", "delta": "hello"}
        });
        let notif: JsonRpcNotification = serde_json::from_value(json).unwrap();
        assert_eq!(notif.method, "agent/message/delta");
        assert!(notif.params.is_some());
    }

    #[test]
    fn thread_start_params_skip_none_fields() {
        let params = ThreadStartParams {
            model: Some("gpt-5.5".into()),
            developer_instructions: Some("You are helpful.".into()),
            ..Default::default()
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["model"], "gpt-5.5");
        assert_eq!(json["developerInstructions"], "You are helpful.");
        // Optional None fields should be absent
        assert!(json.get("cwd").is_none());
        assert!(json.get("personality").is_none());
        assert!(json.get("sandbox").is_none());
    }

    #[test]
    fn thread_fork_params_serializes() {
        let params = ThreadForkParams {
            thread_id: "t_old".into(),
            developer_instructions: Some("Updated persona.".into()),
            exclude_turns: Some(true),
            ..Default::default()
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["threadId"], "t_old");
        assert_eq!(json["developerInstructions"], "Updated persona.");
        assert_eq!(json["excludeTurns"], true);
    }

    #[test]
    fn user_input_text_serializes() {
        let input = UserInput::Text {
            text: "hello".into(),
            text_elements: Vec::new(),
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hello");
        assert_eq!(json["text_elements"], json!([]));
    }

    #[test]
    fn user_input_image_serializes() {
        let input = UserInput::Image {
            url: "data:image/png;base64,abc".into(),
            detail: Some(ImageDetail::High),
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["type"], "image");
        assert_eq!(json["url"], "data:image/png;base64,abc");
        assert_eq!(json["detail"], "high");
    }

    #[test]
    fn user_input_local_image_serializes() {
        let input = UserInput::LocalImage {
            path: "/tmp/img.png".into(),
            detail: None,
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["type"], "localImage");
        assert_eq!(json["path"], "/tmp/img.png");
        assert!(json.get("detail").is_none());
    }

    #[test]
    fn turn_start_params_serializes() {
        let params = TurnStartParams {
            thread_id: "t_1".into(),
            input: vec![UserInput::Text {
                text: "What is Rust?".into(),
                text_elements: Vec::new(),
            }],
            model: None,
            effort: None,
            approval_policy: None,
            cwd: None,
            output_schema: None,
            personality: None,
            service_tier: None,
            summary: None,
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["threadId"], "t_1");
        assert_eq!(json["input"][0]["type"], "text");
        assert_eq!(json["input"][0]["text"], "What is Rust?");
    }

    #[test]
    fn agent_message_delta_deserializes() {
        let json = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "itemId": "item_1",
            "delta": "Hello, world!"
        });
        let notif: AgentMessageDeltaNotification = serde_json::from_value(json).unwrap();
        assert_eq!(notif.thread_id, "t_1");
        assert_eq!(notif.delta, "Hello, world!");
    }

    #[test]
    fn token_usage_deserializes() {
        let json = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "tokenUsage": {
                "last": {
                    "inputTokens": 100,
                    "outputTokens": 50,
                    "cachedInputTokens": 20,
                    "reasoningOutputTokens": 10,
                    "totalTokens": 180
                },
                "total": {
                    "inputTokens": 500,
                    "outputTokens": 200,
                    "cachedInputTokens": 100,
                    "reasoningOutputTokens": 50,
                    "totalTokens": 850
                },
                "modelContextWindow": 128000
            }
        });
        let notif: ThreadTokenUsageUpdatedNotification = serde_json::from_value(json).unwrap();
        assert_eq!(notif.token_usage.last.input_tokens, 100);
        assert_eq!(notif.token_usage.total.total_tokens, 850);
        assert_eq!(notif.token_usage.model_context_window, Some(128000));
    }

    #[test]
    fn token_usage_null_context_window() {
        let json = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "tokenUsage": {
                "last": {
                    "inputTokens": 0, "outputTokens": 0,
                    "cachedInputTokens": 0, "reasoningOutputTokens": 0, "totalTokens": 0
                },
                "total": {
                    "inputTokens": 0, "outputTokens": 0,
                    "cachedInputTokens": 0, "reasoningOutputTokens": 0, "totalTokens": 0
                },
                "modelContextWindow": null
            }
        });
        let notif: ThreadTokenUsageUpdatedNotification = serde_json::from_value(json).unwrap();
        assert_eq!(notif.token_usage.model_context_window, None);
    }

    #[test]
    fn rate_limit_snapshot_deserializes() {
        let json = json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 45,
                    "resetsAt": 1720000000,
                    "windowDurationMins": 60
                },
                "credits": {
                    "hasCredits": true,
                    "balance": "$142.50",
                    "unlimited": false
                },
                "planType": "pro"
            }
        });
        let notif: AccountRateLimitsUpdatedNotification = serde_json::from_value(json).unwrap();
        let primary = notif.rate_limits.primary.unwrap();
        assert_eq!(primary.used_percent, 45);
        assert_eq!(primary.resets_at, Some(1720000000));
        assert_eq!(notif.rate_limits.credits.unwrap().has_credits, true);
        assert_eq!(notif.rate_limits.plan_type, Some(PlanType::Pro));
    }

    #[test]
    fn rate_limit_reached_type_deserializes() {
        let json = json!({
            "rateLimits": {
                "rateLimitReachedType": "rate_limit_reached"
            }
        });
        let notif: AccountRateLimitsUpdatedNotification = serde_json::from_value(json).unwrap();
        assert_eq!(
            notif.rate_limits.rate_limit_reached_type,
            Some(RateLimitReachedType::RateLimitReached)
        );
    }

    #[test]
    fn unknown_rate_limit_type_deserializes() {
        let json = json!({
            "rateLimits": {
                "rateLimitReachedType": "some_future_type"
            }
        });
        let notif: AccountRateLimitsUpdatedNotification = serde_json::from_value(json).unwrap();
        assert_eq!(
            notif.rate_limits.rate_limit_reached_type,
            Some(RateLimitReachedType::Unknown)
        );
    }

    #[test]
    fn error_notification_deserializes() {
        let json = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "error": {
                "message": "Context window exceeded",
                "codexErrorInfo": "contextWindowExceeded"
            },
            "willRetry": false
        });
        let notif: ErrorNotification = serde_json::from_value(json).unwrap();
        assert_eq!(notif.error.message, "Context window exceeded");
        assert_eq!(notif.will_retry, false);
    }

    #[test]
    fn goal_updated_deserializes() {
        let json = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "goal": {
                "threadId": "t_1",
                "objective": "Audit auth module",
                "status": "budgetLimited",
                "tokensUsed": 80000,
                "tokenBudget": 80000,
                "timeUsedSeconds": 120,
                "createdAt": 1720000000,
                "updatedAt": 1720000120
            }
        });
        let notif: ThreadGoalUpdatedNotification = serde_json::from_value(json).unwrap();
        assert_eq!(notif.goal.status, ThreadGoalStatus::BudgetLimited);
        assert_eq!(notif.goal.tokens_used, 80000);
        assert_eq!(notif.goal.token_budget, Some(80000));
    }

    #[test]
    fn approval_review_deserializes() {
        let json = json!({
            "threadId": "t_1",
            "turnId": "turn_1",
            "reviewId": "review_1",
            "action": {"type": "command", "command": "rm -rf /", "cwd": "/home", "source": "shell"},
            "review": {"status": "inProgress"},
            "startedAtMs": 1_720_000_000_000_i64
        });
        let notif: ApprovalReviewStartedNotification = serde_json::from_value(json).unwrap();
        assert_eq!(notif.review_id, "review_1");
        assert_eq!(
            notif.review.status,
            GuardianApprovalReviewStatus::InProgress
        );
    }

    #[test]
    fn mcp_server_status_deserializes() {
        let json = json!({
            "name": "holt",
            "status": "ready"
        });
        let notif: McpServerStatusUpdatedNotification = serde_json::from_value(json).unwrap();
        assert_eq!(notif.name, "holt");
        assert_eq!(notif.status, McpServerStartupState::Ready);
    }

    #[test]
    fn thread_response_deserializes() {
        let json = json!({
            "thread": {
                "id": "t_abc123",
                "sessionId": "s_1",
                "createdAt": 1720000000,
                "updatedAt": 1720000000,
                "cwd": "/home/user/project",
                "modelProvider": "openai",
                "preview": "Hello",
                "status": {"type": "idle"},
                "ephemeral": false,
                "cliVersion": "0.142.5",
                "source": "appServer"
            },
            "model": "gpt-5.5",
            "modelProvider": "openai",
            "cwd": "/home/user/project",
            "approvalPolicy": "never",
            "approvalsReviewer": "user"
        });
        let resp: ThreadResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.thread.id, "t_abc123");
        assert_eq!(resp.model, "gpt-5.5");
    }

    #[test]
    fn thread_with_unknown_fields_deserializes() {
        // Forward compat: unknown fields should not cause errors
        let json = json!({
            "thread": {
                "id": "t_1",
                "sessionId": "s_1",
                "createdAt": 1720000000,
                "updatedAt": 1720000000,
                "cwd": "/tmp",
                "modelProvider": "openai",
                "preview": "",
                "status": {"type": "somethingNew"},
                "ephemeral": false,
                "cliVersion": "0.200.0",
                "source": "future_source",
                "futureField": "should be ignored"
            },
            "model": "gpt-6",
            "modelProvider": "openai",
            "cwd": "/tmp",
            "approvalPolicy": "never",
            "approvalsReviewer": "user",
            "someFutureResponseField": true
        });
        let resp: ThreadResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.thread.id, "t_1");
    }

    #[test]
    fn approval_policy_serializes() {
        assert_eq!(
            serde_json::to_value(ApprovalPolicy::Never).unwrap(),
            "never"
        );
        assert_eq!(
            serde_json::to_value(ApprovalPolicy::OnFailure).unwrap(),
            "on-failure"
        );
    }

    #[test]
    fn thread_goal_status_all_variants() {
        let cases = vec![
            ("active", ThreadGoalStatus::Active),
            ("paused", ThreadGoalStatus::Paused),
            ("blocked", ThreadGoalStatus::Blocked),
            ("usageLimited", ThreadGoalStatus::UsageLimited),
            ("budgetLimited", ThreadGoalStatus::BudgetLimited),
            ("complete", ThreadGoalStatus::Complete),
        ];
        for (json_val, expected) in cases {
            let parsed: ThreadGoalStatus = serde_json::from_value(json!(json_val)).unwrap();
            assert_eq!(parsed, expected);
        }
    }

    #[test]
    fn plan_type_known_variants() {
        let cases = vec![
            ("free", PlanType::Free),
            ("plus", PlanType::Plus),
            ("pro", PlanType::Pro),
            ("team", PlanType::Team),
            ("enterprise", PlanType::Enterprise),
        ];
        for (json_val, expected) in cases {
            let parsed: PlanType = serde_json::from_value(json!(json_val)).unwrap();
            assert_eq!(parsed, expected);
        }
    }

    #[test]
    fn plan_type_unknown_variant() {
        let parsed: PlanType = serde_json::from_value(json!("future_plan")).unwrap();
        assert_eq!(parsed, PlanType::Unknown);
    }

    #[test]
    fn thread_compact_params_serializes() {
        let params = ThreadCompactStartParams {
            thread_id: "t_1".into(),
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["threadId"], "t_1");
    }

    #[test]
    fn thread_rollback_params_serializes() {
        let params = ThreadRollbackParams {
            thread_id: "t_1".into(),
            num_turns: 3,
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["threadId"], "t_1");
        assert_eq!(json["numTurns"], 3);
    }

    #[test]
    fn thread_goal_set_params_serializes() {
        let params = ThreadGoalSetParams {
            thread_id: "t_1".into(),
            objective: Some("Audit auth module".into()),
            token_budget: Some(80000),
            status: None,
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["threadId"], "t_1");
        assert_eq!(json["objective"], "Audit auth module");
        assert_eq!(json["tokenBudget"], 80000);
        assert!(json.get("status").is_none());
    }

    #[test]
    fn thread_inject_items_params_serializes() {
        let params = ThreadInjectItemsParams {
            thread_id: "t_1".into(),
            items: vec![json!({"role": "user", "content": "context from another agent"})],
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["threadId"], "t_1");
        assert_eq!(json["items"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn thread_memory_mode_set_serializes() {
        let params = ThreadMemoryModeSetParams {
            thread_id: "t_1".into(),
            mode: ThreadMemoryMode::Disabled,
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["mode"], "disabled");
    }

    #[test]
    fn turn_interrupt_params_serializes() {
        let params = TurnInterruptParams {
            thread_id: "t_1".into(),
            turn_id: "turn_1".into(),
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["threadId"], "t_1");
        assert_eq!(json["turnId"], "turn_1");
    }

    #[test]
    fn thread_search_params_serializes() {
        let params = ThreadSearchParams {
            search_term: "auth module".into(),
            limit: Some(10),
            cursor: None,
            sort_direction: Some(SortDirection::Desc),
            sort_key: Some(ThreadSortKey::UpdatedAt),
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["searchTerm"], "auth module");
        assert_eq!(json["limit"], 10);
        assert_eq!(json["sortDirection"], "desc");
        assert_eq!(json["sortKey"], "updated_at");
    }
}
