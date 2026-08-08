//! Stream event coalescer — batches events per-agent to reduce frontend event
//! rate during streaming.
//!
//! Design:
//! - Token and Thinking text chunks accumulate in per-agent buffers.
//! - Non-critical events (A2A, ContextWarning, status updates) are deferred
//!   into a per-agent queue and emitted on the same flush interval.
//! - A background flush task drains buffers every `FLUSH_INTERVAL_MS`.
//! - Significant events (ToolCall, Done, Error) force an immediate flush
//!   of ALL pending content (text + deferred) before the event is emitted.
//!
//! Event tiers:
//! - **Immediate**: Done, Error, ToolCall, ToolResult, ConversationUpdated
//!   (user-blocking, latency-sensitive — flush pending then emit)
//! - **Buffered**: Token, Thinking (text coalescing into single chunks)
//! - **Deferred**: A2AMessage, ContextWarning, SandboxStatus, ForkCompleted,
//!   SessionMemoryUpdated, SessionMemoryWarning, SessionMemoryError
//!   (informational — batch on 50ms interval)
//!
//! This replaces per-event emission (~100+ events/sec) with batched emission
//! (~20-30 events/sec), cutting frontend event processing by 4-5x.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::providers::types::{StreamEvent, StreamEventType};

/// Flush interval in milliseconds. Matches the Agent SDK bridge's 50ms batching.
const FLUSH_INTERVAL_MS: u64 = 50;

/// Per-agent buffered content waiting to be flushed.
#[derive(Default)]
struct AgentBuffer {
    /// Accumulated Token text.
    token_text: String,
    /// Accumulated Thinking text.
    thinking_text: String,
    /// Non-critical events deferred until next flush interval.
    deferred_events: Vec<StreamEvent>,
}

/// Shared state between the push path (synchronous) and the flush task (async).
struct CoalescerInner {
    /// Per-agent text buffers.
    buffers: HashMap<String, AgentBuffer>,
}

/// Stream event coalescer that batches Token/Thinking events.
///
/// Create one per application lifetime. The coalescer is `Clone` and thread-safe.
#[derive(Clone)]
pub struct StreamCoalescer {
    inner: Arc<Mutex<CoalescerInner>>,
    /// Emit callback — abstracted so tests can capture output without a real AppHandle.
    emitter: Arc<dyn Fn(StreamEvent) + Send + Sync>,
}

impl StreamCoalescer {
    /// Create a new coalescer that emits events via the given callback.
    pub fn new(emitter: impl Fn(StreamEvent) + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CoalescerInner {
                buffers: HashMap::new(),
            })),
            emitter: Arc::new(emitter),
        }
    }

    /// Create a coalescer wired to a Tauri AppHandle.
    pub fn with_app_handle(app_handle: tauri::AppHandle) -> Self {
        use tauri::Emitter;
        Self::new(move |event: StreamEvent| {
            if let Err(e) = app_handle.emit("agent-stream", &event) {
                tracing::warn!("event emission failed: {e}");
            }
        })
    }

    /// Start the background flush task. Returns a handle that can be aborted
    /// to stop the flush loop (e.g., when the turn ends).
    ///
    /// Call this once per turn or once globally at app startup.
    pub fn start_flush_task(&self) -> tokio::task::JoinHandle<()> {
        let inner = self.inner.clone();
        let emitter = self.emitter.clone();

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_millis(FLUSH_INTERVAL_MS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                interval.tick().await;
                flush_buffers(&inner, &emitter);
            }
        })
    }

    /// Push an event into the coalescer.
    ///
    /// - Token/Thinking: text is buffered and coalesced.
    /// - Immediate events (ToolCall, Done, Error, ToolResult, ConversationUpdated):
    ///   flush all pending content, then emit.
    /// - Deferred events (A2A, ContextWarning, status): queued until next flush.
    pub fn push(&self, event: StreamEvent) {
        match event.event_type {
            // ── Buffered: text coalescing ──────────────────────────────────
            StreamEventType::Token => {
                if let Some(text) = event.data.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                        inner
                            .buffers
                            .entry(event.agent_id.clone())
                            .or_default()
                            .token_text
                            .push_str(text);
                    }
                }
            }
            StreamEventType::Thinking => {
                if let Some(text) = event.data.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                        inner
                            .buffers
                            .entry(event.agent_id.clone())
                            .or_default()
                            .thinking_text
                            .push_str(text);
                    }
                }
            }
            // ── Immediate: flush pending, then emit now ───────────────────
            StreamEventType::ToolCall
            | StreamEventType::Done
            | StreamEventType::Error
            | StreamEventType::ToolResult
            | StreamEventType::ConversationUpdated => {
                // Flush only this agent's buffer before the significant event
                self.flush_agent(&event.agent_id);
                (self.emitter)(event);
            }
            // ── Immediate status: emit without flushing text buffers ─────
            StreamEventType::RateLimitUpdate | StreamEventType::GoalUpdate => {
                (self.emitter)(event);
            }
            // ── Deferred: queue for next flush interval ────────────────────
            StreamEventType::A2AMessage
            | StreamEventType::ContextWarning
            | StreamEventType::SandboxStatus
            | StreamEventType::ForkCompleted
            | StreamEventType::SessionMemoryUpdated
            | StreamEventType::SessionMemoryWarning
            | StreamEventType::SessionMemoryError => {
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                inner
                    .buffers
                    .entry(event.agent_id.clone())
                    .or_default()
                    .deferred_events
                    .push(event);
            }
        }
    }

    /// Flush all pending text for a specific agent. Called before significant events.
    fn flush_agent(&self, agent_id: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(buf) = inner.buffers.get_mut(agent_id) {
            emit_buffer(agent_id, buf, self.emitter.as_ref());
        }
    }

    /// Flush all pending text for all agents. Called at end of turn.
    pub fn flush_all(&self) {
        flush_buffers(&self.inner, &self.emitter);
    }

    /// Remove an agent's buffer (call on agent removal or turn end).
    pub fn remove_agent(&self, agent_id: &str) {
        self.flush_agent(agent_id);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.buffers.remove(agent_id);
    }
}

/// Drain all non-empty agent buffers and emit coalesced events.
fn flush_buffers(
    inner: &Arc<Mutex<CoalescerInner>>,
    emitter: &Arc<dyn Fn(StreamEvent) + Send + Sync>,
) {
    let mut inner = inner.lock().unwrap_or_else(|e| e.into_inner());
    for (agent_id, buf) in inner.buffers.iter_mut() {
        emit_buffer(agent_id, buf, emitter.as_ref());
    }
}

/// Emit any accumulated content for a single agent buffer, then clear it.
/// Order: token text first, thinking text, then deferred events.
fn emit_buffer(agent_id: &str, buf: &mut AgentBuffer, emitter: &dyn Fn(StreamEvent)) {
    if !buf.token_text.is_empty() {
        let text = std::mem::take(&mut buf.token_text);
        emitter(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Token,
            data: serde_json::json!({ "text": text }),
        });
    }
    if !buf.thinking_text.is_empty() {
        let text = std::mem::take(&mut buf.thinking_text);
        emitter(StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Thinking,
            data: serde_json::json!({ "text": text }),
        });
    }
    // Drain deferred events (A2A, status, warnings)
    if !buf.deferred_events.is_empty() {
        for event in buf.deferred_events.drain(..) {
            emitter(event);
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Helper: create a coalescer with a captured output vec.
    fn setup() -> (StreamCoalescer, Arc<Mutex<Vec<StreamEvent>>>) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let c = captured.clone();
        let coalescer = StreamCoalescer::new(move |event| {
            c.lock().unwrap().push(event);
        });
        (coalescer, captured)
    }

    fn token_event(agent_id: &str, text: &str) -> StreamEvent {
        StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Token,
            data: serde_json::json!({ "text": text }),
        }
    }

    fn thinking_event(agent_id: &str, text: &str) -> StreamEvent {
        StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Thinking,
            data: serde_json::json!({ "text": text }),
        }
    }

    fn done_event(agent_id: &str) -> StreamEvent {
        StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Done,
            data: serde_json::json!({}),
        }
    }

    fn tool_call_event(agent_id: &str) -> StreamEvent {
        StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::ToolCall,
            data: serde_json::json!({ "name": "test_tool" }),
        }
    }

    fn error_event(agent_id: &str) -> StreamEvent {
        StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::Error,
            data: serde_json::json!({ "message": "oops" }),
        }
    }

    fn a2a_event(agent_id: &str) -> StreamEvent {
        StreamEvent {
            agent_id: agent_id.to_string(),
            event_type: StreamEventType::A2AMessage,
            data: serde_json::json!({ "from": "other" }),
        }
    }

    #[test]
    fn test_tokens_are_buffered_not_emitted_immediately() {
        let (coalescer, captured) = setup();

        coalescer.push(token_event("agent-1", "Hello"));
        coalescer.push(token_event("agent-1", " world"));

        // Nothing emitted yet ��� tokens are buffered
        assert!(captured.lock().unwrap().is_empty());
    }

    #[test]
    fn test_flush_all_emits_coalesced_token() {
        let (coalescer, captured) = setup();

        coalescer.push(token_event("agent-1", "Hello"));
        coalescer.push(token_event("agent-1", " world"));
        coalescer.push(token_event("agent-1", "!"));

        coalescer.flush_all();

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, StreamEventType::Token);
        assert_eq!(
            events[0].data.get("text").unwrap().as_str().unwrap(),
            "Hello world!"
        );
    }

    #[test]
    fn test_thinking_buffered_separately() {
        let (coalescer, captured) = setup();

        coalescer.push(thinking_event("agent-1", "Let me think"));
        coalescer.push(thinking_event("agent-1", " about this"));
        coalescer.push(token_event("agent-1", "Answer: "));
        coalescer.push(token_event("agent-1", "42"));

        coalescer.flush_all();

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 2); // One Token, one Thinking
        assert_eq!(events[0].event_type, StreamEventType::Token);
        assert_eq!(
            events[0].data.get("text").unwrap().as_str().unwrap(),
            "Answer: 42"
        );
        assert_eq!(events[1].event_type, StreamEventType::Thinking);
        assert_eq!(
            events[1].data.get("text").unwrap().as_str().unwrap(),
            "Let me think about this"
        );
    }

    #[test]
    fn test_tool_call_flushes_before_emit() {
        let (coalescer, captured) = setup();

        coalescer.push(token_event("agent-1", "Thinking..."));
        coalescer.push(tool_call_event("agent-1"));

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 2);
        // Token flushed first
        assert_eq!(events[0].event_type, StreamEventType::Token);
        assert_eq!(
            events[0].data.get("text").unwrap().as_str().unwrap(),
            "Thinking..."
        );
        // Then tool call
        assert_eq!(events[1].event_type, StreamEventType::ToolCall);
    }

    #[test]
    fn test_done_flushes_before_emit() {
        let (coalescer, captured) = setup();

        coalescer.push(token_event("agent-1", "Final answer"));
        coalescer.push(done_event("agent-1"));

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, StreamEventType::Token);
        assert_eq!(events[1].event_type, StreamEventType::Done);
    }

    #[test]
    fn test_error_flushes_before_emit() {
        let (coalescer, captured) = setup();

        coalescer.push(token_event("agent-1", "partial"));
        coalescer.push(error_event("agent-1"));

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, StreamEventType::Token);
        assert_eq!(events[1].event_type, StreamEventType::Error);
    }

    #[test]
    fn test_deferred_events_not_emitted_immediately() {
        let (coalescer, captured) = setup();

        coalescer.push(a2a_event("agent-1"));

        // Deferred events are NOT emitted immediately — they wait for flush
        assert!(captured.lock().unwrap().is_empty());

        // After flush, they come through
        coalescer.flush_all();
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, StreamEventType::A2AMessage);
    }

    #[test]
    fn test_multi_agent_independent_buffers() {
        let (coalescer, captured) = setup();

        coalescer.push(token_event("agent-1", "Hello"));
        coalescer.push(token_event("agent-2", "World"));
        coalescer.push(token_event("agent-1", " there"));

        // Flush only agent-1
        coalescer.flush_agent("agent-1");

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].agent_id, "agent-1");
        assert_eq!(
            events[0].data.get("text").unwrap().as_str().unwrap(),
            "Hello there"
        );
        drop(events);

        // Agent-2 still buffered
        coalescer.flush_all();
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].agent_id, "agent-2");
        assert_eq!(
            events[1].data.get("text").unwrap().as_str().unwrap(),
            "World"
        );
    }

    #[test]
    fn test_empty_text_not_buffered() {
        let (coalescer, captured) = setup();

        coalescer.push(token_event("agent-1", ""));
        coalescer.flush_all();

        assert!(captured.lock().unwrap().is_empty());
    }

    #[test]
    fn test_flush_all_idempotent() {
        let (coalescer, captured) = setup();

        coalescer.push(token_event("agent-1", "test"));
        coalescer.flush_all();
        coalescer.flush_all(); // Second flush should be no-op

        assert_eq!(captured.lock().unwrap().len(), 1);
    }

    #[test]
    fn test_remove_agent_flushes_and_cleans_up() {
        let (coalescer, captured) = setup();

        coalescer.push(token_event("agent-1", "pending"));
        coalescer.remove_agent("agent-1");

        // Should have flushed the pending text
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].data.get("text").unwrap().as_str().unwrap(),
            "pending"
        );
        drop(events);

        // Buffer should be gone — new pushes start fresh
        coalescer.push(token_event("agent-1", "new"));
        coalescer.flush_all();
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].data.get("text").unwrap().as_str().unwrap(), "new");
    }

    #[test]
    fn test_significant_event_only_flushes_that_agent() {
        let (coalescer, captured) = setup();

        coalescer.push(token_event("agent-1", "a1 text"));
        coalescer.push(token_event("agent-2", "a2 text"));

        // Done for agent-1 should only flush agent-1's buffer
        coalescer.push(done_event("agent-1"));

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 2); // a1 token + a1 done
        assert_eq!(events[0].agent_id, "agent-1");
        assert_eq!(events[0].event_type, StreamEventType::Token);
        assert_eq!(events[1].agent_id, "agent-1");
        assert_eq!(events[1].event_type, StreamEventType::Done);
        drop(events);

        // agent-2 still buffered
        coalescer.flush_all();
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[2].agent_id, "agent-2");
    }

    #[tokio::test]
    async fn test_flush_task_drains_on_interval() {
        let (coalescer, captured) = setup();

        coalescer.push(token_event("agent-1", "chunk1"));
        coalescer.push(token_event("agent-1", " chunk2"));

        let task = coalescer.start_flush_task();

        // Wait for at least one tick
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let events = captured.lock().unwrap();
        assert!(!events.is_empty(), "Flush task should have emitted events");
        assert_eq!(events[0].event_type, StreamEventType::Token);
        assert_eq!(
            events[0].data.get("text").unwrap().as_str().unwrap(),
            "chunk1 chunk2"
        );

        task.abort();
    }

    #[test]
    fn test_event_count_reduction() {
        // Simulate 100 individual token events — should coalesce to 1
        let emit_count = Arc::new(AtomicU32::new(0));
        let ec = emit_count.clone();
        let coalescer = StreamCoalescer::new(move |_event| {
            ec.fetch_add(1, Ordering::SeqCst);
        });

        for i in 0..100 {
            coalescer.push(token_event("agent-1", &format!("token{}", i)));
        }

        coalescer.flush_all();

        // 100 tokens → 1 coalesced event
        assert_eq!(emit_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_deferred_events_flushed_before_immediate() {
        let (coalescer, captured) = setup();

        // Queue deferred events
        coalescer.push(a2a_event("agent-1"));
        coalescer.push(StreamEvent {
            agent_id: "agent-1".to_string(),
            event_type: StreamEventType::ContextWarning,
            data: serde_json::json!({ "message": "80% full" }),
        });

        // Nothing emitted yet
        assert!(captured.lock().unwrap().is_empty());

        // Immediate event forces flush of ALL pending (text + deferred)
        coalescer.push(done_event("agent-1"));

        let events = captured.lock().unwrap();
        // Deferred events flushed first, then Done
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event_type, StreamEventType::A2AMessage);
        assert_eq!(events[1].event_type, StreamEventType::ContextWarning);
        assert_eq!(events[2].event_type, StreamEventType::Done);
    }

    #[test]
    fn test_mixed_deferred_and_tokens_flush_together() {
        let (coalescer, captured) = setup();

        coalescer.push(token_event("agent-1", "Hello "));
        coalescer.push(a2a_event("agent-1"));
        coalescer.push(token_event("agent-1", "world"));
        coalescer.push(StreamEvent {
            agent_id: "agent-1".to_string(),
            event_type: StreamEventType::SessionMemoryUpdated,
            data: serde_json::json!({}),
        });

        // Nothing emitted yet
        assert!(captured.lock().unwrap().is_empty());

        coalescer.flush_all();

        let events = captured.lock().unwrap();
        // Token coalesced, then deferred events in order
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event_type, StreamEventType::Token);
        assert_eq!(
            events[0].data.get("text").unwrap().as_str().unwrap(),
            "Hello world"
        );
        assert_eq!(events[1].event_type, StreamEventType::A2AMessage);
        assert_eq!(events[2].event_type, StreamEventType::SessionMemoryUpdated);
    }

    #[test]
    fn test_deferred_events_isolated_per_agent() {
        let (coalescer, captured) = setup();

        coalescer.push(a2a_event("agent-1"));
        coalescer.push(a2a_event("agent-2"));

        // Flush only agent-1
        coalescer.flush_agent("agent-1");

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].agent_id, "agent-1");
        drop(events);

        // Agent-2 still deferred
        coalescer.flush_all();
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].agent_id, "agent-2");
    }
}
