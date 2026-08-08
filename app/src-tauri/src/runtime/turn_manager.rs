//! Per-agent TurnManager — serializes turn execution, owns cancellation tokens,
//! prevents overlapping turns.
//!
//! Every code path that starts agent work (send_message, trigger_agent_turn,
//! broadcast_message, scheduled triggers, A2A wakeups, etc.) submits through
//! the TurnManager. The manager runs a per-agent background worker that
//! processes turn requests sequentially via an mpsc channel.
//!
//! Phase 1: Struct definitions + AppState wiring (no behavioral changes).
//! Phase 2+: Entry points migrated incrementally.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, RwLock};
use tokio_util::sync::CancellationToken;

use crate::runtime::connection::AgentProtocol;

// ── Turn Request Types ──────────────────────────────────────────────────

/// Identifies the protocol-specific execution strategy for a turn.
#[derive(Debug, Clone)]
pub enum TurnProtocol {
    /// HTTP API (OpenAI-compatible, Anthropic, Gemini, XAI, local) → run_agentic_loop
    ApiDirect {
        message: String,
        images: Option<Vec<crate::runtime::agent::ImageAttachment>>,
        content_blocks: Option<Vec<crate::runtime::agent::ContentBlock>>,
        transient_prompt: Option<String>,
        max_turns: Option<usize>,
    },
    /// Claude Agent SDK → agent_sdk_query
    AgentSdk { message: String },
    /// OpenAI Codex CLI → codex_query
    Codex {
        message: String,
        images: Option<Vec<crate::runtime::agent::ImageAttachment>>,
        transient_prompt: Option<String>,
    },
}

impl TurnProtocol {
    /// Determine the correct protocol variant from an AgentProtocol enum.
    /// Returns a builder-style constructor that callers fill in with message data.
    pub fn from_agent_protocol(protocol: &AgentProtocol) -> ProtocolKind {
        match protocol {
            AgentProtocol::AgentSdk => ProtocolKind::AgentSdk,
            AgentProtocol::Codex => ProtocolKind::Codex,
            _ => ProtocolKind::ApiDirect,
        }
    }

    /// Build a TurnProtocol from an AgentProtocol and message data.
    ///
    /// Convenience constructor used by all entry points that need to map
    /// agent protocol + message into the correct TurnProtocol variant.
    pub fn from_message(
        agent_protocol: &AgentProtocol,
        message: String,
        images: Option<Vec<crate::runtime::agent::ImageAttachment>>,
        transient_prompt: Option<String>,
    ) -> Self {
        match Self::from_agent_protocol(agent_protocol) {
            ProtocolKind::AgentSdk => Self::AgentSdk { message },
            ProtocolKind::Codex => Self::Codex {
                message,
                images,
                transient_prompt,
            },
            ProtocolKind::ApiDirect => Self::ApiDirect {
                message,
                images,
                content_blocks: None,
                transient_prompt,
                max_turns: None,
            },
        }
    }
}

/// Simplified protocol discriminant for routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolKind {
    ApiDirect,
    AgentSdk,
    Codex,
}

/// Where the turn request originated (for tracing and debugging).
#[derive(Debug, Clone)]
pub enum TurnSource {
    SendMessage,
    TriggerAgentTurn,
    BroadcastMessage,
    ScheduledTrigger { job_name: String },
    FileWatch { job_name: String },
    A2AMessage { from_agent: String },
}

impl fmt::Display for TurnSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SendMessage => write!(f, "send_message"),
            Self::TriggerAgentTurn => write!(f, "trigger_agent_turn"),
            Self::BroadcastMessage => write!(f, "broadcast_message"),
            Self::ScheduledTrigger { job_name } => write!(f, "scheduled:{job_name}"),
            Self::FileWatch { job_name } => write!(f, "file_watch:{job_name}"),
            Self::A2AMessage { from_agent } => write!(f, "a2a:{from_agent}"),
        }
    }
}

/// Priority levels for turn requests.
/// Higher numeric value = higher priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TurnPriority {
    /// Background: scheduled triggers, file watches, dream cycles
    Background = 0,
    /// Agent-initiated: A2A messages
    AgentInitiated = 1,
    /// User-initiated: send_message, broadcast
    User = 2,
}

/// A request to execute a turn for an agent.
pub struct TurnRequest {
    /// Unique identifier for this turn (UUID).
    pub turn_id: String,
    /// How to execute the turn.
    pub protocol: TurnProtocol,
    /// Who requested it.
    pub source: TurnSource,
    /// Scheduling priority.
    pub priority: TurnPriority,
}

impl TurnRequest {
    /// Create a new turn request with a fresh UUID.
    pub fn new(protocol: TurnProtocol, source: TurnSource, priority: TurnPriority) -> Self {
        Self {
            turn_id: uuid::Uuid::new_v4().to_string(),
            protocol,
            source,
            priority,
        }
    }
}

impl fmt::Debug for TurnRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TurnRequest")
            .field("turn_id", &self.turn_id)
            .field("source", &self.source)
            .field("priority", &self.priority)
            .finish()
    }
}

// ── Turn Result ─────────────────────────────────────────────────────────

/// Outcome sent back to the caller after a turn completes or is cancelled.
#[derive(Debug)]
pub enum TurnResult {
    /// Turn completed successfully, carrying any response content.
    Completed(String),
    /// Turn completed with an error.
    Error(String),
    /// Turn was cancelled before or during execution.
    Cancelled,
    /// Turn was never executed (queue cleared, agent removed).
    Dropped,
}

/// Handle returned to callers who submit a turn request.
/// Drop the handle to fire-and-forget; await `result()` to block on completion.
pub struct TurnHandle {
    pub turn_id: String,
    result_rx: oneshot::Receiver<TurnResult>,
}

impl TurnHandle {
    /// Wait for the turn to complete and return the result.
    pub async fn result(self) -> TurnResult {
        self.result_rx.await.unwrap_or(TurnResult::Dropped)
    }

    /// Check if the turn has completed without blocking.
    pub fn try_result(&mut self) -> Option<TurnResult> {
        self.result_rx.try_recv().ok()
    }
}

// ── Per-Agent Worker ────────────────────────────────────────────────────

/// Queued item: the request + a oneshot sender to deliver the result.
type QueueItem = (TurnRequest, oneshot::Sender<TurnResult>);

/// Per-agent turn processing state.
struct AgentTurnWorker {
    /// Send turn requests into this agent's queue.
    tx: mpsc::Sender<QueueItem>,
    /// Cancellation token for the currently executing turn (if any).
    current_token: Arc<RwLock<Option<CancellationToken>>>,
    /// Turn ID of the currently executing turn (if any).
    current_turn_id: Arc<RwLock<Option<String>>>,
}

// ── TurnManager ─────────────────────────────────────────────────────────

/// Maximum queued turns per agent before new submissions are rejected.
const MAX_QUEUE_DEPTH: usize = 16;

/// Manages turn serialization for all agents.
pub struct TurnManager {
    workers: RwLock<HashMap<String, AgentTurnWorker>>,
}

impl TurnManager {
    pub fn new() -> Self {
        Self {
            workers: RwLock::new(HashMap::new()),
        }
    }

    /// Submit a turn request for an agent.
    ///
    /// If the agent's worker doesn't exist yet, it is created on demand.
    /// Returns a `TurnHandle` that can be awaited for the result.
    ///
    /// Returns `None` if the queue is full (caller should skip or retry).
    pub async fn submit_turn(&self, agent_id: &str, request: TurnRequest) -> Option<TurnHandle> {
        let workers = self.workers.read().await;
        if let Some(worker) = workers.get(agent_id) {
            let (result_tx, result_rx) = oneshot::channel();
            let turn_id = request.turn_id.clone();

            match worker.tx.try_send((request, result_tx)) {
                Ok(()) => {
                    tracing::debug!(
                        agent_id,
                        turn_id = %turn_id,
                        "Turn queued"
                    );
                    Some(TurnHandle { turn_id, result_rx })
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(
                        agent_id,
                        "Turn queue full ({MAX_QUEUE_DEPTH}), rejecting turn request"
                    );
                    None
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    tracing::warn!(
                        agent_id,
                        "Turn worker channel closed, rejecting turn request"
                    );
                    None
                }
            }
        } else {
            drop(workers);
            tracing::warn!(
                agent_id,
                "No turn worker registered for agent, rejecting turn request. \
                 Call ensure_worker() first."
            );
            None
        }
    }

    /// Cancel the currently executing turn for an agent.
    ///
    /// The turn's CancellationToken is cancelled, causing the engine loop to break.
    /// The worker will then process the next queued turn (if any).
    pub async fn cancel_current(&self, agent_id: &str) {
        let workers = self.workers.read().await;
        if let Some(worker) = workers.get(agent_id) {
            let token = worker.current_token.read().await;
            if let Some(ref t) = *token {
                t.cancel();
                tracing::info!(agent_id, "Cancelled current turn via TurnManager");
            }
        }
    }

    /// Cancel the current turn AND drain all queued turns for an agent.
    ///
    /// Used when an agent is being removed or a user wants to fully stop.
    pub async fn cancel_all(&self, agent_id: &str) {
        // First cancel active turn
        self.cancel_current(agent_id).await;

        // Worker's receiver will see cancellation on the active turn.
        // Queued items stay in the channel — they'll be processed but the
        // worker loop should check for a "cancelled" flag or we drain here.
        // For now, cancel_current is sufficient — queued turns will execute
        // after the cancelled turn completes. Full drain requires closing
        // the channel, which we do in remove_worker.
    }

    /// Get the turn ID of the currently executing turn for an agent.
    pub async fn current_turn_id(&self, agent_id: &str) -> Option<String> {
        let workers = self.workers.read().await;
        if let Some(worker) = workers.get(agent_id) {
            worker.current_turn_id.read().await.clone()
        } else {
            None
        }
    }

    /// Get the cancellation token for the currently executing turn.
    ///
    /// Used during migration: callers that still need direct token access
    /// (e.g., interrupt_agent) can get it from here.
    pub async fn current_cancellation_token(&self, agent_id: &str) -> Option<CancellationToken> {
        let workers = self.workers.read().await;
        if let Some(worker) = workers.get(agent_id) {
            worker.current_token.read().await.clone()
        } else {
            None
        }
    }

    /// Check whether an agent currently has an active turn.
    pub async fn is_active(&self, agent_id: &str) -> bool {
        let workers = self.workers.read().await;
        if let Some(worker) = workers.get(agent_id) {
            worker.current_turn_id.read().await.is_some()
        } else {
            false
        }
    }

    /// Register a turn worker for an agent.
    ///
    /// Creates the mpsc channel and background task. If a worker already exists,
    /// this is a no-op.
    ///
    /// The `processor` callback is invoked for each turn request. It receives:
    /// - The agent ID
    /// - The TurnRequest
    /// - A CancellationToken for this turn
    /// And returns a TurnResult.
    ///
    /// In Phase 1 this is called at agent creation time but the processor is a
    /// stub. In Phase 2+ the processor dispatches to the appropriate protocol
    /// handler (run_agentic_loop, agent_sdk_query, codex_query).
    pub async fn ensure_worker<F, Fut>(&self, agent_id: String, processor: F)
    where
        F: Fn(String, TurnRequest, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = TurnResult> + Send + 'static,
    {
        let mut workers = self.workers.write().await;
        if let Some(existing) = workers.get(&agent_id) {
            if !existing.tx.is_closed() {
                return; // Already registered and alive
            }
            // Worker task died (e.g. panicked mid-turn). Fall through and
            // replace it — otherwise the agent rejects every turn until restart.
            tracing::warn!(
                agent_id = %agent_id,
                "Turn worker channel closed — recreating worker"
            );
        }

        let (tx, mut rx) = mpsc::channel::<QueueItem>(MAX_QUEUE_DEPTH);
        let current_token: Arc<RwLock<Option<CancellationToken>>> = Arc::new(RwLock::new(None));
        let current_turn_id: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));

        let ct = current_token.clone();
        let cti = current_turn_id.clone();
        let aid = agent_id.clone();

        // Background worker task: process turns sequentially
        let _task = tokio::spawn(async move {
            while let Some((request, result_tx)) = rx.recv().await {
                let turn_id = request.turn_id.clone();
                let source = request.source.to_string();

                tracing::info!(
                    agent_id = %aid,
                    turn_id = %turn_id,
                    source = %source,
                    "Starting turn"
                );

                // Create cancellation token for this turn
                let token = CancellationToken::new();
                {
                    *ct.write().await = Some(token.clone());
                    *cti.write().await = Some(turn_id.clone());
                }

                // Execute the turn. Catch panics so one bad turn doesn't kill
                // the worker task and strand every subsequent turn for this agent.
                let result = match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
                    processor(aid.clone(), request, token),
                ))
                .await
                {
                    Ok(result) => result,
                    Err(panic) => {
                        let msg = panic
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| panic.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "unknown panic".to_string());
                        tracing::error!(
                            agent_id = %aid,
                            turn_id = %turn_id,
                            panic = %msg,
                            "Turn processor panicked"
                        );
                        TurnResult::Error(format!("Turn panicked: {}", msg))
                    }
                };

                // Clear active turn state
                {
                    *ct.write().await = None;
                    *cti.write().await = None;
                }

                tracing::info!(
                    agent_id = %aid,
                    turn_id = %turn_id,
                    result = ?result,
                    "Turn completed"
                );

                // Deliver result (ignore error if caller dropped the handle)
                let _ = result_tx.send(result);
            }

            tracing::debug!(agent_id = %aid, "Turn worker shutting down (channel closed)");
        });

        workers.insert(
            agent_id,
            AgentTurnWorker {
                tx,
                current_token,
                current_turn_id,
            },
        );
    }

    /// Remove a turn worker for an agent.
    ///
    /// Drops the sender, which causes the worker task to exit after
    /// processing any remaining queued items. Pending callers will
    /// receive `TurnResult::Dropped`.
    pub async fn remove_worker(&self, agent_id: &str) {
        let mut workers = self.workers.write().await;
        if let Some(worker) = workers.remove(agent_id) {
            // Cancel active turn first
            let token = worker.current_token.read().await;
            if let Some(ref t) = *token {
                t.cancel();
            }
            // Dropping `worker.tx` closes the channel, causing the background
            // task to exit after the current turn (if any) completes.
            tracing::debug!(agent_id, "Turn worker removed");
        }
    }

    /// Get the number of registered workers (for diagnostics).
    pub async fn worker_count(&self) -> usize {
        self.workers.read().await.len()
    }
}

impl Default for TurnManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    /// Helper: create a TurnManager with a simple counting processor.
    async fn setup_with_counter(agent_id: &str) -> (Arc<TurnManager>, Arc<AtomicU32>) {
        let tm = Arc::new(TurnManager::new());
        let counter = Arc::new(AtomicU32::new(0));

        let c = counter.clone();
        tm.ensure_worker(agent_id.to_string(), move |_aid, _req, _token| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                TurnResult::Completed(String::new())
            }
        })
        .await;

        (tm, counter)
    }

    fn make_request(source: TurnSource) -> TurnRequest {
        TurnRequest::new(
            TurnProtocol::ApiDirect {
                message: "test".into(),
                images: None,
                content_blocks: None,
                transient_prompt: None,
                max_turns: None,
            },
            source,
            TurnPriority::User,
        )
    }

    #[tokio::test]
    async fn test_panicking_turn_does_not_kill_worker() {
        // Regression: a panic inside the turn processor killed the worker
        // task, closing the queue channel — every subsequent send failed with
        // "Turn queue full or agent not registered" until app restart.
        let tm = Arc::new(TurnManager::new());
        tm.ensure_worker("agent-1".to_string(), move |_aid, req, _token| async move {
            let message = match &req.protocol {
                TurnProtocol::ApiDirect { message, .. } => message.clone(),
                _ => String::new(),
            };
            if message == "test" {
                panic!("boom");
            }
            TurnResult::Completed(message)
        })
        .await;

        // First turn panics — caller should get an error, not a dropped handle
        let handle = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .expect("submit should succeed");
        let result = handle.result().await;
        assert!(
            matches!(&result, TurnResult::Error(e) if e.contains("panicked")),
            "expected panic error, got {:?}",
            result
        );

        // Worker must still be alive and processing turns
        let mut ok_request = make_request(TurnSource::SendMessage);
        ok_request.protocol = TurnProtocol::ApiDirect {
            message: "still alive".into(),
            images: None,
            content_blocks: None,
            transient_prompt: None,
            max_turns: None,
        };
        let handle = tm
            .submit_turn("agent-1", ok_request)
            .await
            .expect("submit after panic should succeed");
        let result = handle.result().await;
        assert!(matches!(&result, TurnResult::Completed(m) if m == "still alive"));
    }

    #[tokio::test]
    async fn test_ensure_worker_replaces_dead_worker() {
        // Even if the worker task dies outright, ensure_worker must replace
        // it instead of treating the dead entry as "already registered".
        let tm = Arc::new(TurnManager::new());

        // Simulate a dead worker: closed channel in the registry
        {
            let (tx, rx) = mpsc::channel::<QueueItem>(MAX_QUEUE_DEPTH);
            drop(rx);
            tm.workers.write().await.insert(
                "agent-1".to_string(),
                AgentTurnWorker {
                    tx,
                    current_token: Arc::new(RwLock::new(None)),
                    current_turn_id: Arc::new(RwLock::new(None)),
                },
            );
        }

        tm.ensure_worker(
            "agent-1".to_string(),
            move |_aid, _req, _token| async move { TurnResult::Completed("recovered".into()) },
        )
        .await;

        let handle = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .expect("submit should succeed after worker replacement");
        let result = handle.result().await;
        assert!(matches!(&result, TurnResult::Completed(m) if m == "recovered"));
    }

    #[tokio::test]
    async fn test_single_turn_completes() {
        let (tm, counter) = setup_with_counter("agent-1").await;

        let handle = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .expect("submit should succeed");

        let result = handle.result().await;
        assert!(matches!(result, TurnResult::Completed(_)));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_sequential_turns() {
        let (tm, counter) = setup_with_counter("agent-1").await;

        let h1 = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .unwrap();
        let h2 = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .unwrap();
        let h3 = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .unwrap();

        // All three should complete sequentially
        assert!(matches!(h1.result().await, TurnResult::Completed(_)));
        assert!(matches!(h2.result().await, TurnResult::Completed(_)));
        assert!(matches!(h3.result().await, TurnResult::Completed(_)));
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_cancel_current_turn() {
        let tm = Arc::new(TurnManager::new());

        tm.ensure_worker("agent-1".to_string(), |_aid, _req, token| async move {
            // Simulate long-running turn that respects cancellation
            tokio::select! {
                _ = token.cancelled() => TurnResult::Cancelled,
                _ = tokio::time::sleep(Duration::from_secs(60)) => TurnResult::Completed(String::new()),
            }
        })
        .await;

        let handle = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .unwrap();

        // Give the worker a moment to start processing
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Cancel it
        tm.cancel_current("agent-1").await;

        let result = handle.result().await;
        assert!(matches!(result, TurnResult::Cancelled));
    }

    #[tokio::test]
    async fn test_no_worker_returns_none() {
        let tm = TurnManager::new();

        let result = tm
            .submit_turn("nonexistent", make_request(TurnSource::SendMessage))
            .await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_ensure_worker_idempotent() {
        let tm = TurnManager::new();

        tm.ensure_worker("agent-1".to_string(), |_, _, _| async {
            TurnResult::Completed(String::new())
        })
        .await;

        tm.ensure_worker("agent-1".to_string(), |_, _, _| async {
            TurnResult::Error("should not be called".into())
        })
        .await;

        // Should use the first worker (idempotent)
        assert_eq!(tm.worker_count().await, 1);
    }

    #[tokio::test]
    async fn test_remove_worker() {
        let (tm, _counter) = setup_with_counter("agent-1").await;

        assert_eq!(tm.worker_count().await, 1);
        tm.remove_worker("agent-1").await;
        assert_eq!(tm.worker_count().await, 0);

        // Submissions should now fail
        let result = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_is_active() {
        let tm = Arc::new(TurnManager::new());

        // Use Notify for Fn-compatible signaling (can be cloned and used multiple times)
        let started = Arc::new(tokio::sync::Notify::new());
        let finish = Arc::new(tokio::sync::Notify::new());

        let s = started.clone();
        let f = finish.clone();
        tm.ensure_worker("agent-1".to_string(), move |_aid, _req, _token| {
            let s = s.clone();
            let f = f.clone();
            async move {
                s.notify_one();
                f.notified().await;
                TurnResult::Completed(String::new())
            }
        })
        .await;

        assert!(!tm.is_active("agent-1").await);

        let handle = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .unwrap();

        // Wait for turn to start
        started.notified().await;
        assert!(tm.is_active("agent-1").await);

        // Let it finish
        finish.notify_one();
        handle.result().await;

        // Small delay for worker to clear state
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!tm.is_active("agent-1").await);
    }

    #[tokio::test]
    async fn test_current_turn_id() {
        let tm = Arc::new(TurnManager::new());

        let started = Arc::new(tokio::sync::Notify::new());
        let finish = Arc::new(tokio::sync::Notify::new());

        let s = started.clone();
        let f = finish.clone();
        tm.ensure_worker("agent-1".to_string(), move |_aid, _req, _token| {
            let s = s.clone();
            let f = f.clone();
            async move {
                s.notify_one();
                f.notified().await;
                TurnResult::Completed(String::new())
            }
        })
        .await;

        assert!(tm.current_turn_id("agent-1").await.is_none());

        let handle = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .unwrap();

        let expected_id = handle.turn_id.clone();
        started.notified().await;

        let active_id = tm.current_turn_id("agent-1").await;
        assert_eq!(active_id, Some(expected_id));

        finish.notify_one();
        handle.result().await;
    }

    #[tokio::test]
    async fn test_turn_error_propagates() {
        let tm = Arc::new(TurnManager::new());

        tm.ensure_worker("agent-1".to_string(), |_aid, _req, _token| async {
            TurnResult::Error("something broke".into())
        })
        .await;

        let handle = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .unwrap();

        match handle.result().await {
            TurnResult::Error(msg) => assert_eq!(msg, "something broke"),
            other => panic!("Expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_multiple_agents_independent() {
        let tm = Arc::new(TurnManager::new());
        let counter_a = Arc::new(AtomicU32::new(0));
        let counter_b = Arc::new(AtomicU32::new(0));

        let ca = counter_a.clone();
        tm.ensure_worker("agent-a".to_string(), move |_, _, _| {
            let ca = ca.clone();
            async move {
                ca.fetch_add(1, Ordering::SeqCst);
                TurnResult::Completed(String::new())
            }
        })
        .await;

        let cb = counter_b.clone();
        tm.ensure_worker("agent-b".to_string(), move |_, _, _| {
            let cb = cb.clone();
            async move {
                cb.fetch_add(1, Ordering::SeqCst);
                TurnResult::Completed(String::new())
            }
        })
        .await;

        let ha = tm
            .submit_turn("agent-a", make_request(TurnSource::SendMessage))
            .await
            .unwrap();
        let hb = tm
            .submit_turn("agent-b", make_request(TurnSource::SendMessage))
            .await
            .unwrap();

        ha.result().await;
        hb.result().await;

        assert_eq!(counter_a.load(Ordering::SeqCst), 1);
        assert_eq!(counter_b.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_from_message_api_direct() {
        let protocol = AgentProtocol::default(); // defaults to something != AgentSdk/Codex
        let tp =
            TurnProtocol::from_message(&protocol, "hello".into(), None, Some("transient".into()));
        match tp {
            TurnProtocol::ApiDirect {
                message,
                images,
                transient_prompt,
                ..
            } => {
                assert_eq!(message, "hello");
                assert!(images.is_none());
                assert_eq!(transient_prompt.as_deref(), Some("transient"));
            }
            other => panic!("Expected ApiDirect, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_from_message_agent_sdk() {
        let protocol = AgentProtocol::AgentSdk;
        let tp = TurnProtocol::from_message(&protocol, "sdk msg".into(), None, None);
        match tp {
            TurnProtocol::AgentSdk { message } => assert_eq!(message, "sdk msg"),
            other => panic!("Expected AgentSdk, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_from_message_codex() {
        let protocol = AgentProtocol::Codex;
        let tp = TurnProtocol::from_message(&protocol, "codex msg".into(), None, None);
        match tp {
            TurnProtocol::Codex { message, .. } => assert_eq!(message, "codex msg"),
            other => panic!("Expected Codex, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_completed_carries_response() {
        let tm = Arc::new(TurnManager::new());

        tm.ensure_worker("agent-1".to_string(), |_aid, _req, _token| async {
            TurnResult::Completed("response content".to_string())
        })
        .await;

        let handle = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .unwrap();

        match handle.result().await {
            TurnResult::Completed(response) => assert_eq!(response, "response content"),
            other => panic!("Expected Completed with response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_cancel_current_with_queued_turns() {
        // Verify that cancelling the current turn lets the next queued turn execute
        let tm = Arc::new(TurnManager::new());
        let counter = Arc::new(AtomicU32::new(0));

        let started = Arc::new(tokio::sync::Notify::new());
        let s = started.clone();
        let c = counter.clone();

        tm.ensure_worker("agent-1".to_string(), move |_aid, _req, token| {
            let s = s.clone();
            let c = c.clone();
            async move {
                let turn_num = c.fetch_add(1, Ordering::SeqCst);
                if turn_num == 0 {
                    // First turn: signal start, then wait for cancellation
                    s.notify_one();
                    tokio::select! {
                        _ = token.cancelled() => TurnResult::Cancelled,
                        _ = tokio::time::sleep(Duration::from_secs(60)) => TurnResult::Completed(String::new()),
                    }
                } else {
                    // Subsequent turns: complete immediately
                    TurnResult::Completed(format!("turn-{}", turn_num))
                }
            }
        })
        .await;

        // Submit two turns
        let h1 = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .unwrap();
        let h2 = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .unwrap();

        // Wait for first turn to start
        started.notified().await;

        // Cancel first turn — second should then execute
        tm.cancel_current("agent-1").await;

        assert!(matches!(h1.result().await, TurnResult::Cancelled));
        match h2.result().await {
            TurnResult::Completed(s) => assert_eq!(s, "turn-1"),
            other => panic!("Expected second turn to complete, got {other:?}"),
        }
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_remove_worker_cancels_active_turn() {
        let tm = Arc::new(TurnManager::new());

        let started = Arc::new(tokio::sync::Notify::new());
        let s = started.clone();

        tm.ensure_worker("agent-1".to_string(), move |_aid, _req, token| {
            let s = s.clone();
            async move {
                s.notify_one();
                tokio::select! {
                    _ = token.cancelled() => TurnResult::Cancelled,
                    _ = tokio::time::sleep(Duration::from_secs(60)) => TurnResult::Completed(String::new()),
                }
            }
        })
        .await;

        let handle = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .unwrap();

        started.notified().await;

        // Remove worker should cancel the active turn
        tm.remove_worker("agent-1").await;

        assert!(matches!(handle.result().await, TurnResult::Cancelled));
        assert_eq!(tm.worker_count().await, 0);
    }

    #[tokio::test]
    async fn test_current_cancellation_token() {
        let tm = Arc::new(TurnManager::new());

        let started = Arc::new(tokio::sync::Notify::new());
        let finish = Arc::new(tokio::sync::Notify::new());
        let s = started.clone();
        let f = finish.clone();

        tm.ensure_worker("agent-1".to_string(), move |_aid, _req, _token| {
            let s = s.clone();
            let f = f.clone();
            async move {
                s.notify_one();
                f.notified().await;
                TurnResult::Completed(String::new())
            }
        })
        .await;

        // No turn active — token should be None
        assert!(tm.current_cancellation_token("agent-1").await.is_none());

        let handle = tm
            .submit_turn("agent-1", make_request(TurnSource::SendMessage))
            .await
            .unwrap();

        started.notified().await;

        // Turn active — should have a token
        let token = tm.current_cancellation_token("agent-1").await;
        assert!(token.is_some());
        assert!(!token.unwrap().is_cancelled());

        finish.notify_one();
        handle.result().await;
    }

    #[tokio::test]
    async fn test_turn_source_display() {
        assert_eq!(TurnSource::SendMessage.to_string(), "send_message");
        assert_eq!(
            TurnSource::TriggerAgentTurn.to_string(),
            "trigger_agent_turn"
        );
        assert_eq!(
            TurnSource::BroadcastMessage.to_string(),
            "broadcast_message"
        );
        assert_eq!(
            TurnSource::ScheduledTrigger {
                job_name: "daily".into()
            }
            .to_string(),
            "scheduled:daily"
        );
        assert_eq!(
            TurnSource::FileWatch {
                job_name: "config".into()
            }
            .to_string(),
            "file_watch:config"
        );
        assert_eq!(
            TurnSource::A2AMessage {
                from_agent: "demo".into()
            }
            .to_string(),
            "a2a:demo"
        );
    }

    #[tokio::test]
    async fn test_priority_ordering() {
        assert!(TurnPriority::Background < TurnPriority::AgentInitiated);
        assert!(TurnPriority::AgentInitiated < TurnPriority::User);
    }
}
