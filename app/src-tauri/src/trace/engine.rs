use chrono::{DateTime, Utc};
use std::path::Path;

use super::store::{TraceRow, TraceStore};
use super::truncation::{truncate_trace_content, TraceOutcome, TraceType};

pub use super::store::DailyUsage;

/// High-level trace engine that wraps TraceStore with truncation and convenience methods.
pub struct TraceEngine {
    store: TraceStore,
}

/// Input for writing a trace record.
pub struct TraceInput {
    pub agent_id: String,
    pub trace_type: TraceType,
    pub content: serde_json::Value,
    pub outcome: Option<TraceOutcome>,
    pub duration_ms: Option<u64>,
    pub token_count: Option<u32>,
    pub parent_trace_id: Option<String>,
    pub session_id: String,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub cost_usd_cents: Option<i64>,
}

/// A structured trace record returned from queries.
#[derive(Debug, Clone)]
pub struct TraceRecord {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub agent_id: String,
    pub trace_type: TraceType,
    pub content: serde_json::Value,
    pub outcome: Option<TraceOutcome>,
    pub duration_ms: Option<u64>,
    pub token_count: Option<u32>,
    pub parent_trace_id: Option<String>,
    pub session_id: String,
}

impl TraceEngine {
    /// Create a new TraceEngine backed by a SQLite database at the given path.
    pub fn new(db_path: &Path) -> Result<Self, rusqlite::Error> {
        let store = TraceStore::new(db_path)?;
        Ok(Self { store })
    }

    /// Create an in-memory TraceEngine for testing.
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, rusqlite::Error> {
        let store = TraceStore::in_memory()?;
        Ok(Self { store })
    }

    /// Write a trace record. Content is truncated per TraceType limits before storage.
    /// Returns the trace ID.
    pub fn write_trace(&self, input: TraceInput) -> Result<String, Box<dyn std::error::Error>> {
        let id = uuid::Uuid::new_v4().to_string();
        self.write_trace_with_id(id, input)
    }

    /// Write a trace record with a caller-provided trace ID.
    /// Returns the trace ID.
    pub fn write_trace_with_id(
        &self,
        id: String,
        input: TraceInput,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let timestamp = Utc::now().to_rfc3339();

        // Truncate content before storing
        let content_str = input.content.to_string();
        let truncated = truncate_trace_content(&input.trace_type, &content_str);

        let outcome_str = input
            .outcome
            .map(|o| serde_json::to_string(&o).unwrap_or_default());

        self.store
            .insert_trace(&crate::trace::store::InsertTraceParams {
                id: &id,
                timestamp: &timestamp,
                agent_id: &input.agent_id,
                trace_type: input.trace_type.as_str(),
                content: &truncated,
                outcome: outcome_str.as_deref(),
                duration_ms: input.duration_ms,
                token_count: input.token_count,
                parent_trace_id: input.parent_trace_id.as_deref(),
                session_id: &input.session_id,
                prompt_tokens: input.prompt_tokens,
                completion_tokens: input.completion_tokens,
                cost_usd_cents: input.cost_usd_cents,
            })?;

        Ok(id)
    }

    /// Read a single trace by its ID.
    pub fn read_trace(
        &self,
        trace_id: &str,
    ) -> Result<Option<TraceRecord>, Box<dyn std::error::Error>> {
        let row = self.store.query_by_id(trace_id)?;
        Ok(row.map(row_to_record))
    }

    /// Query traces by agent ID.
    pub fn query_by_agent(
        &self,
        agent_id: &str,
        limit: u32,
    ) -> Result<Vec<TraceRecord>, Box<dyn std::error::Error>> {
        let rows = self.store.query_by_agent(agent_id, limit)?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    /// Query traces by type.
    pub fn query_by_type(
        &self,
        trace_type: &TraceType,
        limit: u32,
    ) -> Result<Vec<TraceRecord>, Box<dyn std::error::Error>> {
        let rows = self.store.query_by_type(trace_type.as_str(), limit)?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    /// Query traces by session ID.
    pub fn query_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<TraceRecord>, Box<dyn std::error::Error>> {
        let rows = self.store.query_by_session(session_id)?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    /// Query traces within a time range.
    pub fn query_by_time_range(
        &self,
        start: &DateTime<Utc>,
        end: &DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<TraceRecord>, Box<dyn std::error::Error>> {
        let rows = self
            .store
            .query_by_time_range(&start.to_rfc3339(), &end.to_rfc3339(), limit)?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    /// Update the outcome of a trace record.
    pub fn update_outcome(
        &self,
        trace_id: &str,
        outcome: &TraceOutcome,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let outcome_str = serde_json::to_string(outcome)?;
        self.store.update_outcome(trace_id, &outcome_str)?;
        Ok(())
    }

    /// Create a new trace session. Returns the session ID.
    pub fn create_session(&self, agent_id: &str) -> Result<String, Box<dyn std::error::Error>> {
        let id = uuid::Uuid::new_v4().to_string();
        let started_at = Utc::now().to_rfc3339();
        self.store.create_session(&id, agent_id, &started_at)?;
        Ok(id)
    }

    /// End a trace session.
    pub fn end_session(
        &self,
        session_id: &str,
        outcome: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let ended_at = Utc::now().to_rfc3339();
        self.store.end_session(session_id, &ended_at, outcome)?;
        Ok(())
    }

    /// Query the most recent traces across all agents.
    pub fn query_recent(&self, limit: u32) -> Result<Vec<TraceRecord>, Box<dyn std::error::Error>> {
        let rows = self.store.query_recent(limit)?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    /// Query recent traces as raw serializable rows.
    pub fn query_recent_rows(
        &self,
        limit: u32,
    ) -> Result<Vec<TraceRow>, Box<dyn std::error::Error>> {
        let rows = self.store.query_recent(limit)?;
        Ok(rows)
    }

    /// Query traces for a specific agent as raw serializable rows.
    pub fn query_by_agent_rows(
        &self,
        agent_id: &str,
        limit: u32,
    ) -> Result<Vec<TraceRow>, Box<dyn std::error::Error>> {
        let rows = self.store.query_by_agent(agent_id, limit)?;
        Ok(rows)
    }

    /// Get total trace count.
    pub fn count_traces(&self) -> Result<u64, rusqlite::Error> {
        self.store.count_traces()
    }

    /// Delete traces older than `retention_days` days.
    /// Returns the number of deleted traces.
    pub fn cleanup_retention(
        &self,
        retention_days: u32,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        let cutoff = Utc::now() - chrono::Duration::days(retention_days as i64);
        let cutoff_str = cutoff.to_rfc3339();
        let deleted = self.store.delete_older_than(&cutoff_str)?;
        Ok(deleted)
    }

    /// Clear all traces.
    pub fn clear_all(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.store.clear_all()?;
        Ok(())
    }

    /// Query daily token usage for the last N days.
    pub fn query_daily_usage(
        &self,
        days: u32,
    ) -> Result<Vec<DailyUsage>, Box<dyn std::error::Error>> {
        self.store.query_daily_usage(days).map_err(|e| e.into())
    }

    /// Query total cost for a specific agent session.
    pub fn query_session_cost(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> Result<i64, Box<dyn std::error::Error>> {
        self.store
            .query_session_cost(agent_id, session_id)
            .map_err(|e| e.into())
    }

    /// Flush WAL to main database file.
    pub fn flush_wal(&self) -> Result<(), rusqlite::Error> {
        self.store.flush_wal()
    }

    /// Run a passive WAL checkpoint. Called on a 2s cadence by the
    /// background task spawned in `start_background_tasks` so that
    /// TraceTile and ActivityTile queries see recent writes.
    pub fn checkpoint(&self) -> Result<(), rusqlite::Error> {
        self.store.checkpoint()
    }
}

fn row_to_record(row: TraceRow) -> TraceRecord {
    TraceRecord {
        id: row.id,
        timestamp: DateTime::parse_from_rfc3339(&row.timestamp)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        agent_id: row.agent_id,
        trace_type: row.trace_type.parse().unwrap_or(TraceType::Error),
        content: serde_json::from_str(&row.content)
            .unwrap_or(serde_json::Value::String(row.content)),
        outcome: row.outcome.and_then(|s| serde_json::from_str(&s).ok()),
        duration_ms: row.duration_ms.map(|v| v as u64),
        token_count: row.token_count.map(|v| v as u32),
        parent_trace_id: row.parent_trace_id,
        session_id: row.session_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_query_trace() {
        let engine = TraceEngine::in_memory().unwrap();
        let session_id = engine.create_session("agent-001").unwrap();

        let trace_id = engine
            .write_trace(TraceInput {
                agent_id: "agent-001".to_string(),
                trace_type: TraceType::HilMessage,
                content: serde_json::json!({"message": "Hello, agent!"}),
                outcome: Some(TraceOutcome::Success),
                duration_ms: None,
                token_count: None,
                parent_trace_id: None,
                session_id: session_id.clone(),
                prompt_tokens: None,
                completion_tokens: None,
                cost_usd_cents: None,
            })
            .unwrap();

        assert!(!trace_id.is_empty());

        let results = engine.query_by_agent("agent-001", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].agent_id, "agent-001");
    }

    #[test]
    fn test_query_by_type() {
        let engine = TraceEngine::in_memory().unwrap();
        let session_id = engine.create_session("agent-001").unwrap();

        // Write 3 tool calls and 2 agent responses
        for i in 0..3 {
            engine
                .write_trace(TraceInput {
                    agent_id: "agent-001".to_string(),
                    trace_type: TraceType::ToolCall,
                    content: serde_json::json!({"tool": format!("tool_{}", i)}),
                    outcome: Some(TraceOutcome::Success),
                    duration_ms: Some(100),
                    token_count: None,
                    parent_trace_id: None,
                    session_id: session_id.clone(),
                    prompt_tokens: None,
                    completion_tokens: None,
                    cost_usd_cents: None,
                })
                .unwrap();
        }
        for i in 0..2 {
            engine
                .write_trace(TraceInput {
                    agent_id: "agent-001".to_string(),
                    trace_type: TraceType::AgentResponse,
                    content: serde_json::json!({"response": format!("resp_{}", i)}),
                    outcome: Some(TraceOutcome::Success),
                    duration_ms: Some(500),
                    token_count: Some(200),
                    parent_trace_id: None,
                    session_id: session_id.clone(),
                    prompt_tokens: None,
                    completion_tokens: None,
                    cost_usd_cents: None,
                })
                .unwrap();
        }

        let tool_calls = engine.query_by_type(&TraceType::ToolCall, 100).unwrap();
        assert_eq!(tool_calls.len(), 3);

        let responses = engine
            .query_by_type(&TraceType::AgentResponse, 100)
            .unwrap();
        assert_eq!(responses.len(), 2);
    }

    #[test]
    fn test_insert_1000_traces() {
        let engine = TraceEngine::in_memory().unwrap();
        let session_id = engine.create_session("agent-bench").unwrap();

        for i in 0..1000 {
            engine
                .write_trace(TraceInput {
                    agent_id: "agent-bench".to_string(),
                    trace_type: if i % 3 == 0 {
                        TraceType::ToolCall
                    } else if i % 3 == 1 {
                        TraceType::ToolResult
                    } else {
                        TraceType::AgentResponse
                    },
                    content: serde_json::json!({"index": i, "data": "x".repeat(500)}),
                    outcome: Some(TraceOutcome::Success),
                    duration_ms: Some(i as u64),
                    token_count: Some((i * 10) as u32),
                    parent_trace_id: None,
                    session_id: session_id.clone(),
                    prompt_tokens: None,
                    completion_tokens: None,
                    cost_usd_cents: None,
                })
                .unwrap();
        }

        let count = engine.count_traces().unwrap();
        assert_eq!(count, 1000);

        // Query by type
        let tool_calls = engine.query_by_type(&TraceType::ToolCall, 1000).unwrap();
        assert_eq!(tool_calls.len(), 334); // 0, 3, 6, ... 999 → 334 items

        let responses = engine
            .query_by_type(&TraceType::AgentResponse, 1000)
            .unwrap();
        assert_eq!(responses.len(), 333);
    }

    #[test]
    fn test_truncation_applied_on_write() {
        let engine = TraceEngine::in_memory().unwrap();
        let session_id = engine.create_session("agent-001").unwrap();

        // ToolCall limit is 1000 chars — write 3000 chars of content
        let big_content = serde_json::json!({"data": "y".repeat(3000)});
        engine
            .write_trace(TraceInput {
                agent_id: "agent-001".to_string(),
                trace_type: TraceType::ToolCall,
                content: big_content,
                outcome: None,
                duration_ms: None,
                token_count: None,
                parent_trace_id: None,
                session_id,
                prompt_tokens: None,
                completion_tokens: None,
                cost_usd_cents: None,
            })
            .unwrap();

        let results = engine.query_by_agent("agent-001", 1).unwrap();
        assert_eq!(results.len(), 1);
        // Content should have been truncated
        let content_str = results[0].content.to_string();
        assert!(content_str.contains("TRUNCATED"));
    }

    #[test]
    fn test_update_outcome() {
        let engine = TraceEngine::in_memory().unwrap();
        let session_id = engine.create_session("agent-001").unwrap();

        let trace_id = engine
            .write_trace(TraceInput {
                agent_id: "agent-001".to_string(),
                trace_type: TraceType::ToolCall,
                content: serde_json::json!({"tool": "bash"}),
                outcome: Some(TraceOutcome::Unknown),
                duration_ms: None,
                token_count: None,
                parent_trace_id: None,
                session_id,
                prompt_tokens: None,
                completion_tokens: None,
                cost_usd_cents: None,
            })
            .unwrap();

        engine
            .update_outcome(
                &trace_id,
                &TraceOutcome::Failure {
                    reason: "timeout".to_string(),
                },
            )
            .unwrap();

        let results = engine.query_by_agent("agent-001", 1).unwrap();
        match &results[0].outcome {
            Some(TraceOutcome::Failure { reason }) => assert_eq!(reason, "timeout"),
            other => panic!("Expected Failure outcome, got {:?}", other),
        }
    }

    #[test]
    fn test_session_lifecycle() {
        let engine = TraceEngine::in_memory().unwrap();
        let session_id = engine.create_session("agent-001").unwrap();
        assert!(!session_id.is_empty());

        engine.end_session(&session_id, "success").unwrap();
    }

    #[test]
    fn query_session_cost_sums_traces_for_session() {
        let engine = TraceEngine::in_memory().unwrap();
        let sess1 = engine.create_session("agent-1").unwrap();
        let sess2 = engine.create_session("agent-1").unwrap();

        // Two traces in session 1
        engine
            .write_trace(TraceInput {
                agent_id: "agent-1".to_string(),
                trace_type: TraceType::AgentResponse,
                content: serde_json::json!({"text": "response 1"}),
                outcome: Some(TraceOutcome::Success),
                duration_ms: Some(100),
                token_count: None,
                parent_trace_id: None,
                session_id: sess1.clone(),
                prompt_tokens: Some(1000),
                completion_tokens: Some(500),
                cost_usd_cents: Some(25),
            })
            .unwrap();

        engine
            .write_trace(TraceInput {
                agent_id: "agent-1".to_string(),
                trace_type: TraceType::AgentResponse,
                content: serde_json::json!({"text": "response 2"}),
                outcome: Some(TraceOutcome::Success),
                duration_ms: Some(100),
                token_count: None,
                parent_trace_id: None,
                session_id: sess1.clone(),
                prompt_tokens: Some(2000),
                completion_tokens: Some(800),
                cost_usd_cents: Some(50),
            })
            .unwrap();

        // One trace in session 2 — should not be counted
        engine
            .write_trace(TraceInput {
                agent_id: "agent-1".to_string(),
                trace_type: TraceType::AgentResponse,
                content: serde_json::json!({"text": "response 3"}),
                outcome: Some(TraceOutcome::Success),
                duration_ms: Some(100),
                token_count: None,
                parent_trace_id: None,
                session_id: sess2.clone(),
                prompt_tokens: Some(500),
                completion_tokens: Some(200),
                cost_usd_cents: Some(10),
            })
            .unwrap();

        let cost = engine.query_session_cost("agent-1", &sess1).unwrap();
        assert_eq!(cost, 75); // 25 + 50

        let cost_other = engine.query_session_cost("agent-1", &sess2).unwrap();
        assert_eq!(cost_other, 10);

        let cost_empty = engine
            .query_session_cost("agent-1", "nonexistent-session")
            .unwrap();
        assert_eq!(cost_empty, 0);
    }
}
