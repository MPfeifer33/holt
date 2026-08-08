use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

/// Parameters for inserting a trace record.
pub struct InsertTraceParams<'a> {
    pub id: &'a str,
    pub timestamp: &'a str,
    pub agent_id: &'a str,
    pub trace_type: &'a str,
    pub content: &'a str,
    pub outcome: Option<&'a str>,
    pub duration_ms: Option<u64>,
    pub token_count: Option<u32>,
    pub parent_trace_id: Option<&'a str>,
    pub session_id: &'a str,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub cost_usd_cents: Option<i64>,
}

/// Daily token usage aggregation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DailyUsage {
    pub date: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cost_usd_cents: i64,
}

/// SQLite-backed trace storage (PRD Section 6.2).
pub struct TraceStore {
    conn: Mutex<Connection>,
}

impl TraceStore {
    /// Open or create the trace database at the given path.
    pub fn new(db_path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let conn = Connection::open(db_path)?;

        // Enable WAL mode for concurrent read/write performance
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

        // Create tables and indexes
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS traces (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                trace_type TEXT NOT NULL,
                content TEXT NOT NULL,
                outcome TEXT,
                duration_ms INTEGER,
                token_count INTEGER,
                parent_trace_id TEXT,
                session_id TEXT NOT NULL,
                created_at TEXT DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_traces_agent ON traces(agent_id, timestamp);
            CREATE INDEX IF NOT EXISTS idx_traces_session ON traces(session_id);
            CREATE INDEX IF NOT EXISTS idx_traces_type ON traces(trace_type);
            CREATE INDEX IF NOT EXISTS idx_traces_outcome ON traces(outcome);

            CREATE TABLE IF NOT EXISTS trace_sessions (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                task_description TEXT,
                overall_outcome TEXT,
                trace_count INTEGER DEFAULT 0,
                error_count INTEGER DEFAULT 0
            );
            ",
        )?;

        // Migrate: add token tracking columns (v0.2.0+)
        let has_prompt_tokens: bool = conn
            .prepare("SELECT prompt_tokens FROM traces LIMIT 0")
            .is_ok();
        if !has_prompt_tokens {
            conn.execute_batch(
                "ALTER TABLE traces ADD COLUMN prompt_tokens INTEGER;
                 ALTER TABLE traces ADD COLUMN completion_tokens INTEGER;
                 ALTER TABLE traces ADD COLUMN cost_usd_cents INTEGER;
                 CREATE INDEX IF NOT EXISTS idx_traces_created_at ON traces(created_at);",
            )?;
        }

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Create an in-memory store for testing.
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS traces (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                trace_type TEXT NOT NULL,
                content TEXT NOT NULL,
                outcome TEXT,
                duration_ms INTEGER,
                token_count INTEGER,
                parent_trace_id TEXT,
                session_id TEXT NOT NULL,
                created_at TEXT DEFAULT (datetime('now')),
                prompt_tokens INTEGER,
                completion_tokens INTEGER,
                cost_usd_cents INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_traces_agent ON traces(agent_id, timestamp);
            CREATE INDEX IF NOT EXISTS idx_traces_session ON traces(session_id);
            CREATE INDEX IF NOT EXISTS idx_traces_type ON traces(trace_type);
            CREATE INDEX IF NOT EXISTS idx_traces_outcome ON traces(outcome);
            CREATE INDEX IF NOT EXISTS idx_traces_created_at ON traces(created_at);

            CREATE TABLE IF NOT EXISTS trace_sessions (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                task_description TEXT,
                overall_outcome TEXT,
                trace_count INTEGER DEFAULT 0,
                error_count INTEGER DEFAULT 0
            );
            ",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn insert_trace(&self, p: &InsertTraceParams<'_>) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO traces (id, timestamp, agent_id, trace_type, content, outcome, duration_ms, token_count, parent_trace_id, session_id, prompt_tokens, completion_tokens, cost_usd_cents)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                p.id,
                p.timestamp,
                p.agent_id,
                p.trace_type,
                p.content,
                p.outcome,
                p.duration_ms.map(|v| v as i64),
                p.token_count.map(|v| v as i32),
                p.parent_trace_id,
                p.session_id,
                p.prompt_tokens.map(|v| v as i32),
                p.completion_tokens.map(|v| v as i32),
                p.cost_usd_cents,
            ],
        )?;
        Ok(())
    }

    /// Query a single trace by ID.
    pub fn query_by_id(&self, trace_id: &str) -> Result<Option<TraceRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, agent_id, trace_type, content, outcome, duration_ms, token_count, parent_trace_id, session_id
             FROM traces WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![trace_id], TraceRow::from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn query_by_agent(
        &self,
        agent_id: &str,
        limit: u32,
    ) -> Result<Vec<TraceRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, agent_id, trace_type, content, outcome, duration_ms, token_count, parent_trace_id, session_id
             FROM traces WHERE agent_id = ?1 ORDER BY timestamp DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![agent_id, limit], TraceRow::from_row)?;
        rows.collect()
    }

    pub fn query_by_type(
        &self,
        trace_type: &str,
        limit: u32,
    ) -> Result<Vec<TraceRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, agent_id, trace_type, content, outcome, duration_ms, token_count, parent_trace_id, session_id
             FROM traces WHERE trace_type = ?1 ORDER BY timestamp DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![trace_type, limit], TraceRow::from_row)?;
        rows.collect()
    }

    pub fn query_by_session(&self, session_id: &str) -> Result<Vec<TraceRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, agent_id, trace_type, content, outcome, duration_ms, token_count, parent_trace_id, session_id
             FROM traces WHERE session_id = ?1 ORDER BY timestamp ASC",
        )?;
        let rows = stmt.query_map(params![session_id], TraceRow::from_row)?;
        rows.collect()
    }

    pub fn query_by_time_range(
        &self,
        start: &str,
        end: &str,
        limit: u32,
    ) -> Result<Vec<TraceRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, agent_id, trace_type, content, outcome, duration_ms, token_count, parent_trace_id, session_id
             FROM traces WHERE timestamp >= ?1 AND timestamp <= ?2 ORDER BY timestamp ASC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![start, end, limit], TraceRow::from_row)?;
        rows.collect()
    }

    pub fn update_outcome(&self, trace_id: &str, outcome: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE traces SET outcome = ?1 WHERE id = ?2",
            params![outcome, trace_id],
        )?;
        Ok(())
    }

    pub fn create_session(
        &self,
        id: &str,
        agent_id: &str,
        started_at: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO trace_sessions (id, agent_id, started_at) VALUES (?1, ?2, ?3)",
            params![id, agent_id, started_at],
        )?;
        Ok(())
    }

    pub fn end_session(
        &self,
        session_id: &str,
        ended_at: &str,
        outcome: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE trace_sessions SET ended_at = ?1, overall_outcome = ?2 WHERE id = ?3",
            params![ended_at, outcome, session_id],
        )?;
        Ok(())
    }

    pub fn count_traces(&self) -> Result<u64, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM traces", [], |row| {
            row.get::<_, i64>(0).map(|v| v as u64)
        })
    }

    /// Query the most recent traces across all agents.
    pub fn query_recent(&self, limit: u32) -> Result<Vec<TraceRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, agent_id, trace_type, content, outcome, duration_ms, token_count, parent_trace_id, session_id
             FROM traces ORDER BY timestamp DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], TraceRow::from_row)?;
        rows.collect()
    }

    /// Delete traces older than the given RFC 3339 cutoff timestamp.
    /// Returns the number of deleted rows.
    pub fn delete_older_than(&self, cutoff_rfc3339: &str) -> Result<u64, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute(
            "DELETE FROM traces WHERE timestamp < ?1",
            params![cutoff_rfc3339],
        )?;
        Ok(deleted as u64)
    }

    /// Delete all traces.
    pub fn clear_all(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("DELETE FROM traces; DELETE FROM trace_sessions;")?;
        Ok(())
    }

    /// Query daily token usage for the last N days.
    pub fn query_daily_usage(&self, days: u32) -> Result<Vec<DailyUsage>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT date(created_at) as day, SUM(prompt_tokens), SUM(completion_tokens), SUM(cost_usd_cents)
             FROM traces
             WHERE trace_type = 'AgentResponse' AND prompt_tokens IS NOT NULL
             GROUP BY day ORDER BY day DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![days], |row| {
            Ok(DailyUsage {
                date: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                prompt_tokens: row.get::<_, Option<i64>>(1)?.unwrap_or(0) as u64,
                completion_tokens: row.get::<_, Option<i64>>(2)?.unwrap_or(0) as u64,
                cost_usd_cents: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
            })
        })?;
        rows.collect()
    }

    /// Query total cost for a specific agent session.
    pub fn query_session_cost(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> Result<i64, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT COALESCE(SUM(cost_usd_cents), 0) FROM traces
             WHERE agent_id = ?1 AND session_id = ?2 AND cost_usd_cents IS NOT NULL",
        )?;
        let cost: i64 = stmt.query_row(params![agent_id, session_id], |row| row.get(0))?;
        Ok(cost)
    }

    /// Flush WAL to main database file (used during shutdown).
    pub fn flush_wal(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    /// Passive WAL checkpoint — does not block readers or writers.
    /// Called periodically by a background task so that read queries
    /// (TraceTile, ActivityTile backfill) see recent writes without
    /// having to wait for shutdown's flush_wal.
    pub fn checkpoint(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(())
    }
}

/// Raw row from the traces table.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TraceRow {
    pub id: String,
    pub timestamp: String,
    pub agent_id: String,
    pub trace_type: String,
    pub content: String,
    pub outcome: Option<String>,
    pub duration_ms: Option<i64>,
    pub token_count: Option<i32>,
    pub parent_trace_id: Option<String>,
    pub session_id: String,
}

impl TraceRow {
    fn from_row(row: &rusqlite::Row<'_>) -> Result<Self, rusqlite::Error> {
        Ok(Self {
            id: row.get(0)?,
            timestamp: row.get(1)?,
            agent_id: row.get(2)?,
            trace_type: row.get(3)?,
            content: row.get(4)?,
            outcome: row.get(5)?,
            duration_ms: row.get(6)?,
            token_count: row.get(7)?,
            parent_trace_id: row.get(8)?,
            session_id: row.get(9)?,
        })
    }
}
