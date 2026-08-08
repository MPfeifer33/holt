//! SQLite implementation of MemoryStore.
//!
//! Source of truth for all memory data. HNSW is a derived index rebuilt from here.
//! Uses rusqlite with bundled SQLite. Embeddings stored as blobs (f32 little-endian).

use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

use super::{MemoryStore, StoreError};
use crate::model::*;

/// SQLite-backed memory store.
///
/// Uses a Mutex<Connection> because rusqlite's Connection is !Send.
/// For V1 single-process usage this is fine. If contention becomes
/// an issue, move to a connection pool or r2d2.
pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    /// Open or create a SQLite database at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn = Connection::open(path)
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("sqlite open: {e}")))?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Create an in-memory database (for testing).
    pub fn in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("sqlite open: {e}")))?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        conn.execute_batch(SCHEMA_SQL)
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("schema init: {e}")))?;

        Self::migrate_superseded_at(&conn)?;
        Self::migrate_last_decayed_at(&conn)?;
        Self::migrate_content_hash_unique(&conn)?;

        // Backfill FTS index from any existing data (idempotent)
        conn.execute_batch(
            "INSERT OR IGNORE INTO memories_fts(rowid, content)
             SELECT rowid, content FROM memories WHERE deleted_at IS NULL",
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("fts backfill: {e}")))?;

        Ok(())
    }

    /// Add `memories.superseded_at` to databases created before it existed.
    ///
    /// SQLite has no `ADD COLUMN IF NOT EXISTS`, so the column is probed first. Idempotent.
    ///
    /// **Backfill:** existing superseded rows are stamped with the migration time rather than
    /// their `created_at`. When they were actually superseded is not recorded anywhere — the
    /// absence of that record is the bug this column fixes — and `created_at` is precisely the
    /// wrong answer, since using it is what made the retention window zero. Stamping "now" is
    /// the conservative reading: every already-superseded memory gets a full retention window
    /// starting at the migration, so the migration itself cannot make anything deletable that
    /// was not already deletable a moment before.
    /// Add a column to `memories` if it is missing. Returns true if it was added.
    ///
    /// SQLite has no `ADD COLUMN IF NOT EXISTS`, so the column is probed first.
    fn ensure_memories_column(
        conn: &rusqlite::Connection,
        column: &str,
        decl: &str,
    ) -> Result<bool, StoreError> {
        let exists = conn
            .prepare("SELECT 1 FROM pragma_table_info('memories') WHERE name = ?1")
            .and_then(|mut stmt| stmt.exists([column]))
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("probe {column}: {e}")))?;

        if exists {
            return Ok(false);
        }

        conn.execute_batch(&format!("ALTER TABLE memories ADD COLUMN {column} {decl}"))
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("add {column}: {e}")))?;

        Ok(true)
    }

    /// Add `memories.last_decayed_at` to databases created before it existed.
    ///
    /// Left NULL for existing rows: NULL means "never decayed by the interval-aware path",
    /// which permits one decay on the next run. Backfilling to now would instead skip a
    /// cycle for the entire store.
    fn migrate_last_decayed_at(conn: &rusqlite::Connection) -> Result<(), StoreError> {
        if Self::ensure_memories_column(conn, "last_decayed_at", "TEXT")? {
            tracing::info!("migrated memories.last_decayed_at");
        }
        Ok(())
    }

    /// Enforce one live memory per (space, content_hash) at the database level.
    ///
    /// Partial on `deleted_at IS NULL AND is_chunk = 0`, which matters twice: soft-deleted
    /// duplicates do not block the index (that is how the existing 11 were collapsed), and
    /// chunks are exempt because a chunk's hash is not meaningfully unique — two documents can
    /// legitimately share an identical fragment.
    ///
    /// **Skips rather than fails when duplicates exist.** `init_schema` runs on every open, so
    /// an unconditional `CREATE UNIQUE INDEX` against a store that still has duplicates would
    /// error out of `SqliteStore::open` and make the database unopenable — turning a cosmetic
    /// data problem into total unavailability. The application-level dedup in `ops::remember`
    /// is the primary guard; this index is defence in depth, so degrading to "not installed"
    /// is the correct failure mode.
    fn migrate_content_hash_unique(conn: &rusqlite::Connection) -> Result<(), StoreError> {
        let dupes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM (
                   SELECT 1 FROM memories
                    WHERE deleted_at IS NULL AND is_chunk = 0
                    GROUP BY space_id, content_hash HAVING COUNT(*) > 1)",
                [],
                |row| row.get(0),
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("probe duplicate hashes: {e}")))?;

        if dupes > 0 {
            tracing::warn!(
                duplicate_groups = dupes,
                "skipping content_hash unique index: live duplicates present. Collapse them \
                 (keep the oldest row per group) and reopen to install it."
            );
            return Ok(());
        }

        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_memories_content_hash_live
                 ON memories(space_id, content_hash)
              WHERE deleted_at IS NULL AND is_chunk = 0",
        )
        .map_err(|e| {
            StoreError::Internal(anyhow::anyhow!("create content_hash unique index: {e}"))
        })?;

        Ok(())
    }

    fn migrate_superseded_at(conn: &rusqlite::Connection) -> Result<(), StoreError> {
        if !Self::ensure_memories_column(conn, "superseded_at", "TEXT")? {
            return Ok(());
        }

        let backfilled = conn
            .execute(
                "UPDATE memories
                    SET superseded_at = ?1
                  WHERE belief_state = 'superseded'
                    AND superseded_at IS NULL",
                params![Utc::now().to_rfc3339()],
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("backfill superseded_at: {e}")))?;

        tracing::info!(
            backfilled,
            "migrated memories.superseded_at; existing superseded rows given a full retention \
             window from now"
        );

        Ok(())
    }
}

const SCHEMA_SQL: &str = r#"
    PRAGMA journal_mode = WAL;
    PRAGMA foreign_keys = ON;

    CREATE TABLE IF NOT EXISTS spaces (
        id              TEXT PRIMARY KEY,
        rotation_kind   TEXT NOT NULL,       -- 'identity' or 'derived'
        rotation_seed   BLOB,               -- encrypted at rest (V1: plaintext, see security stance)
        created_at      TEXT NOT NULL,
        memory_count    INTEGER NOT NULL DEFAULT 0
    );

    CREATE TABLE IF NOT EXISTS memories (
        id              TEXT PRIMARY KEY,    -- UUID as text
        content         TEXT NOT NULL,
        embedding       BLOB,               -- f32 little-endian, NULL for parent memories
        space_id        TEXT NOT NULL REFERENCES spaces(id),

        -- Metadata
        kind            TEXT NOT NULL,
        source          TEXT NOT NULL,
        tags            TEXT NOT NULL DEFAULT '[]',  -- JSON array
        created_at      TEXT NOT NULL,
        content_hash    BLOB NOT NULL,       -- 32 bytes BLAKE3

        -- Retained signals (inactive for ranking)
        importance      REAL,
        confidence      REAL,
        belief_state    TEXT,                -- 'active', 'superseded', 'uncertain'
        superseded_at   TEXT,                -- when belief_state became 'superseded'; NULL otherwise.
                                             -- Retention is measured from THIS, not created_at.
        last_decayed_at TEXT,                -- last importance decay. NULL = never decayed.
                                             -- Makes decay idempotent within an interval.

        -- Chunking
        parent_id       TEXT REFERENCES memories(id),
        chunk_index     INTEGER,
        is_chunk        INTEGER NOT NULL DEFAULT 0,

        -- Provenance
        source_agent    TEXT,
        source_memory   TEXT,
        source_space    TEXT,
        shared_at       TEXT,

        -- Soft delete
        deleted_at      TEXT
    );

    CREATE INDEX IF NOT EXISTS idx_memories_space ON memories(space_id) WHERE deleted_at IS NULL;
    CREATE INDEX IF NOT EXISTS idx_memories_parent ON memories(parent_id) WHERE parent_id IS NOT NULL;
    CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);
    CREATE INDEX IF NOT EXISTS idx_memories_kind ON memories(kind) WHERE deleted_at IS NULL;

    CREATE TABLE IF NOT EXISTS key_ring (
        agent_id        TEXT NOT NULL,
        space_id        TEXT NOT NULL REFERENCES spaces(id),
        granted_at      TEXT NOT NULL,
        granted_by      TEXT NOT NULL,
        PRIMARY KEY (agent_id, space_id)
    );

    -- Pinned memories (cap enforced at engine level: see ops::pin::MAX_PINS)
    CREATE TABLE IF NOT EXISTS pinned_memories (
        agent_id    TEXT NOT NULL,
        memory_id   TEXT NOT NULL,
        pinned_at   TEXT NOT NULL,
        PRIMARY KEY (agent_id, memory_id)
    );

    -- Full-text search index (BM25)
    CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
        content,
        tokenize='porter unicode61'
    );
"#;

// ── Trait Implementation ────────────────────────────────────────────

#[async_trait]
impl MemoryStore for SqliteStore {
    async fn insert_memory(&self, memory: &Memory) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let embedding_blob = memory.embedding.as_ref().map(|e| f32s_to_bytes(e));
        let tags_json = serde_json::to_string(&memory.tags).unwrap_or_else(|_| "[]".to_string());
        let belief_str = memory.belief_state.map(belief_to_str);

        conn.execute(
            "INSERT INTO memories (
                id, content, embedding, space_id,
                kind, source, tags, created_at, content_hash,
                importance, confidence, belief_state,
                parent_id, chunk_index, is_chunk,
                source_agent, source_memory, source_space, shared_at
            ) VALUES (
                ?1, ?2, ?3, ?4,
                ?5, ?6, ?7, ?8, ?9,
                ?10, ?11, ?12,
                ?13, ?14, ?15,
                ?16, ?17, ?18, ?19
            )",
            params![
                memory.id.to_string(),
                memory.content,
                embedding_blob,
                memory.space_id,
                kind_to_str(memory.kind),
                source_to_str(memory.source),
                tags_json,
                memory.created_at.to_rfc3339(),
                memory.content_hash.as_slice(),
                memory.importance,
                memory.confidence,
                belief_str,
                memory.parent_id.map(|u| u.to_string()),
                memory.chunk_index,
                memory.is_chunk as i32,
                memory.source_agent,
                memory.source_memory.map(|u| u.to_string()),
                memory.source_space,
                memory.shared_at.map(|t| t.to_rfc3339()),
            ],
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("insert memory: {e}")))?;

        // Update space memory count
        conn.execute(
            "UPDATE spaces SET memory_count = memory_count + 1 WHERE id = ?1",
            params![memory.space_id],
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("update space count: {e}")))?;

        // Sync FTS5 index
        conn.execute(
            "INSERT INTO memories_fts(rowid, content)
             SELECT rowid, content FROM memories WHERE id = ?1",
            params![memory.id.to_string()],
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("fts insert: {e}")))?;

        Ok(())
    }

    async fn get_memory(&self, id: Uuid) -> Result<Memory, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, content, embedding, space_id,
                        kind, source, tags, created_at, content_hash,
                        importance, confidence, belief_state,
                        parent_id, chunk_index, is_chunk,
                        source_agent, source_memory, source_space, shared_at
                 FROM memories WHERE id = ?1 AND deleted_at IS NULL",
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("prepare: {e}")))?;

        stmt.query_row(params![id.to_string()], |row| Ok(row_to_memory(row)))
            .optional()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("get memory: {e}")))?
            .ok_or(StoreError::NotFound(id))
    }

    async fn get_chunks(&self, parent_id: Uuid) -> Result<Vec<Memory>, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, content, embedding, space_id,
                        kind, source, tags, created_at, content_hash,
                        importance, confidence, belief_state,
                        parent_id, chunk_index, is_chunk,
                        source_agent, source_memory, source_space, shared_at
                 FROM memories
                 WHERE parent_id = ?1 AND deleted_at IS NULL
                 ORDER BY chunk_index",
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("prepare: {e}")))?;

        let memories = stmt
            .query_map(params![parent_id.to_string()], |row| Ok(row_to_memory(row)))
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("get chunks: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("collect chunks: {e}")))?;

        Ok(memories)
    }

    async fn get_parent(&self, chunk_id: Uuid) -> Result<Memory, StoreError> {
        let mem = self.get_memory(chunk_id).await?;
        let parent_id = mem
            .parent_id
            .ok_or_else(|| StoreError::Internal(anyhow::anyhow!("memory is not a chunk")))?;
        self.get_memory(parent_id).await
    }

    async fn delete_memory(&self, id: Uuid) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;

        // Remove from FTS before soft-delete
        conn.execute(
            "DELETE FROM memories_fts WHERE rowid = (SELECT rowid FROM memories WHERE id = ?1)",
            params![id.to_string()],
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("fts delete: {e}")))?;

        let now = Utc::now().to_rfc3339();
        let affected = conn
            .execute(
                "UPDATE memories SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
                params![now, id.to_string()],
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("delete memory: {e}")))?;

        if affected == 0 {
            return Err(StoreError::NotFound(id));
        }
        Ok(())
    }

    async fn delete_parent_and_chunks(&self, parent_id: Uuid) -> Result<u32, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let now = Utc::now().to_rfc3339();

        // Remove from FTS before soft-delete
        conn.execute(
            "DELETE FROM memories_fts WHERE rowid IN (
                SELECT rowid FROM memories WHERE (id = ?1 OR parent_id = ?1) AND deleted_at IS NULL
            )",
            params![parent_id.to_string()],
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("fts delete cascade: {e}")))?;

        // Soft-delete chunks
        let chunks_deleted = conn
            .execute(
                "UPDATE memories SET deleted_at = ?1 WHERE parent_id = ?2 AND deleted_at IS NULL",
                params![now, parent_id.to_string()],
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("delete chunks: {e}")))?;

        // Soft-delete parent
        let parent_deleted = conn
            .execute(
                "UPDATE memories SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
                params![now, parent_id.to_string()],
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("delete parent: {e}")))?;

        Ok((chunks_deleted + parent_deleted) as u32)
    }

    async fn is_chunk(&self, id: Uuid) -> Result<bool, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let result: Option<bool> = conn
            .query_row(
                "SELECT is_chunk FROM memories WHERE id = ?1 AND deleted_at IS NULL",
                params![id.to_string()],
                |row| row.get::<_, i32>(0).map(|v| v != 0),
            )
            .optional()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("is_chunk: {e}")))?;

        result.ok_or(StoreError::NotFound(id))
    }

    async fn is_parent(&self, id: Uuid) -> Result<bool, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE parent_id = ?1 AND deleted_at IS NULL",
                params![id.to_string()],
                |row| row.get(0),
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("is_parent: {e}")))?;

        Ok(count > 0)
    }

    async fn update_belief_state(
        &self,
        ids: &[Uuid],
        state: BeliefState,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let state_str = belief_to_str(state);

        // `superseded_at` starts the retention clock, so it is maintained here rather than
        // inferred later. COALESCE keeps the original timestamp if a memory is superseded
        // again — re-supersession must not push its deletion indefinitely into the future.
        // Any other state clears it, taking the memory back off the prune path entirely.
        for id in ids {
            let result = if state == BeliefState::Superseded {
                conn.execute(
                    "UPDATE memories
                        SET belief_state = ?1, superseded_at = COALESCE(superseded_at, ?3)
                      WHERE id = ?2 AND deleted_at IS NULL",
                    params![state_str, id.to_string(), Utc::now().to_rfc3339()],
                )
            } else {
                conn.execute(
                    "UPDATE memories
                        SET belief_state = ?1, superseded_at = NULL
                      WHERE id = ?2 AND deleted_at IS NULL",
                    params![state_str, id.to_string()],
                )
            };
            result.map_err(|e| StoreError::Internal(anyhow::anyhow!("update belief: {e}")))?;
        }

        Ok(())
    }

    async fn update_importance(&self, id: Uuid, importance: f32) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        conn.execute(
            "UPDATE memories SET importance = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            params![importance, id.to_string()],
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("update importance: {e}")))?;
        Ok(())
    }

    async fn all_vectors_in_space(
        &self,
        space_id: &str,
    ) -> Result<Vec<(Uuid, Vec<f32>)>, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, embedding FROM memories
                 WHERE space_id = ?1 AND embedding IS NOT NULL AND deleted_at IS NULL",
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("prepare: {e}")))?;

        let results = stmt
            .query_map(params![space_id], |row| {
                let id_str: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((
                    Uuid::parse_str(&id_str).unwrap_or_default(),
                    bytes_to_f32s(&blob),
                ))
            })
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("all_vectors: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("collect vectors: {e}")))?;

        Ok(results)
    }

    async fn count_memories(&self, space_id: &str) -> Result<u64, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE space_id = ?1 AND deleted_at IS NULL",
                params![space_id],
                |row| row.get(0),
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("count: {e}")))?;
        Ok(count as u64)
    }

    async fn count_tombstones(&self) -> Result<u64, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE deleted_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("count tombstones: {e}")))?;
        Ok(count as u64)
    }

    async fn growth_history(&self, days: u32) -> Result<Vec<GrowthPoint>, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let mut stmt = conn
            .prepare(
                "SELECT DATE(created_at) as day, COUNT(*) as cnt
                 FROM memories
                 WHERE created_at >= DATE('now', ?1)
                   AND deleted_at IS NULL
                 GROUP BY day
                 ORDER BY day",
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("prepare growth: {e}")))?;

        let offset = format!("-{days} days");
        let results = stmt
            .query_map(params![offset], |row| {
                let day_str: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok(GrowthPoint {
                    date: NaiveDateTime::parse_from_str(
                        &format!("{day_str} 00:00:00"),
                        "%Y-%m-%d %H:%M:%S",
                    )
                    .unwrap_or_default()
                    .and_utc(),
                    count: count as u64,
                })
            })
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("growth: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("collect growth: {e}")))?;

        Ok(results)
    }

    async fn create_space(&self, space: &Space) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let rotation_kind = match space.rotation_kind {
            RotationKind::Identity => "identity",
            RotationKind::Derived => "derived",
        };

        conn.execute(
            "INSERT INTO spaces (id, rotation_kind, rotation_seed, created_at, memory_count)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                space.id,
                rotation_kind,
                space.rotation_seed.as_ref().map(|s| s.as_slice()),
                space.created_at.to_rfc3339(),
                space.memory_count as i64,
            ],
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("create space: {e}")))?;

        Ok(())
    }

    async fn get_space(&self, id: &str) -> Result<Space, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        conn.query_row(
            "SELECT id, rotation_kind, rotation_seed, created_at, memory_count
             FROM spaces WHERE id = ?1",
            params![id],
            |row| {
                Ok(Space {
                    id: row.get(0)?,
                    rotation_kind: match row.get::<_, String>(1)?.as_str() {
                        "derived" => RotationKind::Derived,
                        _ => RotationKind::Identity,
                    },
                    rotation_seed: row.get::<_, Option<Vec<u8>>>(2)?.and_then(|v| {
                        if v.len() == 32 {
                            let mut arr = [0u8; 32];
                            arr.copy_from_slice(&v);
                            Some(arr)
                        } else {
                            None
                        }
                    }),
                    created_at: parse_timestamp(&row.get::<_, String>(3)?),
                    memory_count: row.get::<_, i64>(4)? as u64,
                })
            },
        )
        .optional()
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("get space: {e}")))?
        .ok_or_else(|| StoreError::Internal(anyhow::anyhow!("space not found: {id}")))
    }

    async fn list_spaces(&self) -> Result<Vec<Space>, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, rotation_kind, rotation_seed, created_at, memory_count FROM spaces",
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("prepare: {e}")))?;

        let spaces = stmt
            .query_map([], |row| {
                Ok(Space {
                    id: row.get(0)?,
                    rotation_kind: match row.get::<_, String>(1)?.as_str() {
                        "derived" => RotationKind::Derived,
                        _ => RotationKind::Identity,
                    },
                    rotation_seed: row.get::<_, Option<Vec<u8>>>(2)?.and_then(|v| {
                        if v.len() == 32 {
                            let mut arr = [0u8; 32];
                            arr.copy_from_slice(&v);
                            Some(arr)
                        } else {
                            None
                        }
                    }),
                    created_at: parse_timestamp(&row.get::<_, String>(3)?),
                    memory_count: row.get::<_, i64>(4)? as u64,
                })
            })
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("list spaces: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("collect spaces: {e}")))?;

        Ok(spaces)
    }

    async fn get_key_ring(&self, agent_id: &str) -> Result<Vec<KeyRingEntry>, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let mut stmt = conn
            .prepare(
                "SELECT agent_id, space_id, granted_at, granted_by
                 FROM key_ring WHERE agent_id = ?1",
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("prepare: {e}")))?;

        let entries = stmt
            .query_map(params![agent_id], |row| {
                Ok(KeyRingEntry {
                    agent_id: row.get(0)?,
                    space_id: row.get(1)?,
                    granted_at: parse_timestamp(&row.get::<_, String>(2)?),
                    granted_by: row.get(3)?,
                })
            })
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("get key_ring: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("collect key_ring: {e}")))?;

        Ok(entries)
    }

    async fn grant_access(&self, entry: &KeyRingEntry) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        conn.execute(
            "INSERT OR REPLACE INTO key_ring (agent_id, space_id, granted_at, granted_by)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                entry.agent_id,
                entry.space_id,
                entry.granted_at.to_rfc3339(),
                entry.granted_by,
            ],
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("grant access: {e}")))?;
        Ok(())
    }

    async fn revoke_access(&self, agent_id: &str, space_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        conn.execute(
            "DELETE FROM key_ring WHERE agent_id = ?1 AND space_id = ?2",
            params![agent_id, space_id],
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("revoke access: {e}")))?;
        Ok(())
    }

    async fn restore_superseded(&self, space_id: Option<&str>) -> Result<u32, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        // Both columns, deliberately: leaving a stale `superseded_at` behind on a row that
        // reads as active is exactly the kind of inconsistent state that later confuses a
        // reader about what actually happened.
        let n = match space_id {
            Some(sid) => conn.execute(
                "UPDATE memories SET belief_state = 'active', superseded_at = NULL
                 WHERE belief_state = 'superseded' AND deleted_at IS NULL AND space_id = ?1",
                params![sid],
            ),
            None => conn.execute(
                "UPDATE memories SET belief_state = 'active', superseded_at = NULL
                 WHERE belief_state = 'superseded' AND deleted_at IS NULL",
                [],
            ),
        }
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("restore_superseded: {e}")))?;
        Ok(n as u32)
    }

    async fn count_superseded(&self, space_id: Option<&str>) -> Result<u32, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let n: i64 = match space_id {
            Some(sid) => conn.query_row(
                "SELECT COUNT(*) FROM memories
                 WHERE belief_state = 'superseded' AND deleted_at IS NULL AND space_id = ?1",
                params![sid],
                |r| r.get(0),
            ),
            None => conn.query_row(
                "SELECT COUNT(*) FROM memories
                 WHERE belief_state = 'superseded' AND deleted_at IS NULL",
                [],
                |r| r.get(0),
            ),
        }
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("count_superseded: {e}")))?;
        Ok(n as u32)
    }

    async fn agents_with_access(&self, space_id: &str) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let mut stmt = conn
            .prepare("SELECT DISTINCT agent_id FROM key_ring WHERE space_id = ?1 ORDER BY agent_id")
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("agents_with_access: {e}")))?;

        let ids = stmt
            .query_map(params![space_id], |row| row.get::<_, String>(0))
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("agents_with_access query: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                StoreError::Internal(anyhow::anyhow!("agents_with_access collect: {e}"))
            })?;

        Ok(ids)
    }

    async fn filter_memories(
        &self,
        space_ids: &[String],
        filter: &FilterInput,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<FilterResult>, u64), StoreError> {
        if space_ids.is_empty() {
            return Ok((Vec::new(), 0));
        }

        let conn = self.conn.lock().map_err(lock_err)?;

        // Build WHERE clause dynamically
        let mut conditions = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut param_idx = 1usize;

        // Space filter
        let space_placeholders: String = space_ids
            .iter()
            .map(|sid| {
                let p = format!("?{param_idx}");
                params_vec.push(Box::new(sid.clone()));
                param_idx += 1;
                p
            })
            .collect::<Vec<_>>()
            .join(", ");
        conditions.push(format!("m.space_id IN ({space_placeholders})"));

        // Always exclude deleted and chunks
        conditions.push("m.deleted_at IS NULL".to_string());
        conditions.push("m.is_chunk = 0".to_string());

        if let Some(kind) = &filter.kind {
            conditions.push(format!("m.kind = ?{param_idx}"));
            params_vec.push(Box::new(kind_to_str(*kind).to_string()));
            param_idx += 1;
        }
        if let Some(source) = &filter.source {
            conditions.push(format!("m.source = ?{param_idx}"));
            params_vec.push(Box::new(source_to_str(*source).to_string()));
            param_idx += 1;
        }
        if let Some(belief) = &filter.belief_state {
            conditions.push(format!("m.belief_state = ?{param_idx}"));
            params_vec.push(Box::new(belief_to_str(*belief).to_string()));
            param_idx += 1;
        }
        if let Some(imp_min) = filter.importance_min {
            conditions.push(format!("m.importance >= ?{param_idx}"));
            params_vec.push(Box::new(imp_min as f64));
            param_idx += 1;
        }
        if let Some(conf_min) = filter.confidence_min {
            conditions.push(format!("m.confidence >= ?{param_idx}"));
            params_vec.push(Box::new(conf_min as f64));
            param_idx += 1;
        }
        if let Some(date_start) = &filter.date_start {
            conditions.push(format!("m.created_at >= ?{param_idx}"));
            params_vec.push(Box::new(date_start.to_rfc3339()));
            param_idx += 1;
        }
        if let Some(date_end) = &filter.date_end {
            conditions.push(format!("m.created_at <= ?{param_idx}"));
            params_vec.push(Box::new(date_end.to_rfc3339()));
            param_idx += 1;
        }

        // Tag filter (any match): check if JSON array contains any of the requested tags
        for tag in &filter.tags {
            conditions.push(format!("m.tags LIKE ?{param_idx}"));
            params_vec.push(Box::new(format!("%\"{}\"%", tag)));
            param_idx += 1;
        }

        // Pinned filter
        if let Some(pinned) = filter.pinned {
            if pinned {
                conditions.push("p.memory_id IS NOT NULL".to_string());
            } else {
                conditions.push("p.memory_id IS NULL".to_string());
            }
        }

        let where_clause = conditions.join(" AND ");

        let sort_col = match filter.sort_by {
            SortField::CreatedAt => "m.created_at",
            SortField::Importance => "COALESCE(m.importance, 0)",
        };
        let sort_dir = match filter.sort_order {
            SortOrder::Desc => "DESC",
            SortOrder::Asc => "ASC",
        };

        // Count query
        let count_sql = format!(
            "SELECT COUNT(*)
             FROM memories m
             LEFT JOIN pinned_memories p ON m.id = p.memory_id
             WHERE {where_clause}"
        );

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let total_count: i64 = conn
            .query_row(&count_sql, param_refs.as_slice(), |row| row.get(0))
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("filter count: {e}")))?;

        // Data query
        let data_sql = format!(
            "SELECT m.id, m.content, m.kind, m.source, m.tags, m.created_at,
                    m.importance, m.confidence, m.belief_state, m.space_id,
                    CASE WHEN p.memory_id IS NOT NULL THEN 1 ELSE 0 END as is_pinned
             FROM memories m
             LEFT JOIN pinned_memories p ON m.id = p.memory_id
             WHERE {where_clause}
             ORDER BY {sort_col} {sort_dir}
             LIMIT ?{param_idx} OFFSET ?{}",
            param_idx + 1
        );

        let mut data_params = params_vec;
        data_params.push(Box::new(limit as i64));
        data_params.push(Box::new(offset as i64));

        let data_refs: Vec<&dyn rusqlite::types::ToSql> =
            data_params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&data_sql)
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("filter prepare: {e}")))?;

        let results = stmt
            .query_map(data_refs.as_slice(), |row| {
                let id_str: String = row.get(0)?;
                let content: String = row.get(1)?;
                let kind_str: String = row.get(2)?;
                let source_str: String = row.get(3)?;
                let tags_json: String = row.get(4)?;
                let created_at_str: String = row.get(5)?;
                let importance: Option<f64> = row.get(6)?;
                let confidence: Option<f64> = row.get(7)?;
                let belief_str: Option<String> = row.get(8)?;
                let space_id: String = row.get(9)?;
                let is_pinned: i32 = row.get(10)?;

                Ok(FilterResult {
                    memory_id: Uuid::parse_str(&id_str).unwrap_or_default(),
                    content,
                    kind: str_to_kind(&kind_str),
                    source: str_to_source(&source_str),
                    tags: serde_json::from_str(&tags_json).unwrap_or_default(),
                    created_at: parse_timestamp(&created_at_str),
                    importance: importance.map(|v| v as f32),
                    confidence: confidence.map(|v| v as f32),
                    belief_state: belief_str.as_deref().map(str_to_belief),
                    pinned: is_pinned != 0,
                    space_id,
                })
            })
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("filter query: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("filter collect: {e}")))?;

        Ok((results, total_count as u64))
    }

    async fn pin_memory(&self, agent_id: &str, memory_id: Uuid) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        conn.execute(
            "INSERT OR IGNORE INTO pinned_memories (agent_id, memory_id, pinned_at)
             VALUES (?1, ?2, ?3)",
            params![agent_id, memory_id.to_string(), Utc::now().to_rfc3339()],
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("pin memory: {e}")))?;
        Ok(())
    }

    async fn unpin_memory(&self, agent_id: &str, memory_id: Uuid) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        conn.execute(
            "DELETE FROM pinned_memories WHERE agent_id = ?1 AND memory_id = ?2",
            params![agent_id, memory_id.to_string()],
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("unpin memory: {e}")))?;
        Ok(())
    }

    async fn get_pinned(&self, agent_id: &str) -> Result<Vec<Uuid>, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let mut stmt = conn
            .prepare("SELECT memory_id FROM pinned_memories WHERE agent_id = ?1 ORDER BY pinned_at")
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("prepare pinned: {e}")))?;

        let ids = stmt
            .query_map(params![agent_id], |row| {
                let id_str: String = row.get(0)?;
                Ok(Uuid::parse_str(&id_str).unwrap_or_default())
            })
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("get pinned: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("collect pinned: {e}")))?;

        Ok(ids)
    }

    async fn is_pinned(&self, agent_id: &str, memory_id: Uuid) -> Result<bool, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pinned_memories WHERE agent_id = ?1 AND memory_id = ?2",
                params![agent_id, memory_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("is_pinned: {e}")))?;
        Ok(count > 0)
    }

    async fn fts_search(
        &self,
        query: &str,
        space_ids: &[String],
        limit: usize,
    ) -> Result<Vec<FtsResult>, StoreError> {
        if space_ids.is_empty() || query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().map_err(lock_err)?;

        // Sanitize query: quote each token for literal matching
        let sanitized: String = query
            .split_whitespace()
            .map(|token| {
                let clean: String = token
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == '\'')
                    .collect();
                if clean.is_empty() {
                    String::new()
                } else {
                    format!("\"{clean}\"")
                }
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");

        if sanitized.is_empty() {
            return Ok(Vec::new());
        }

        // Build dynamic IN clause
        let placeholders: String = (0..space_ids.len())
            .map(|i| format!("?{}", i + 3))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "SELECT m.id, m.space_id, -bm25(memories_fts) as score
             FROM memories_fts
             JOIN memories m ON m.rowid = memories_fts.rowid
             WHERE memories_fts MATCH ?1
               AND m.deleted_at IS NULL
               AND (m.belief_state IS NULL OR m.belief_state != 'superseded')
               AND m.space_id IN ({placeholders})
             ORDER BY score DESC
             LIMIT ?2"
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("fts prepare: {e}")))?;

        // Bind parameters: ?1=query, ?2=limit, ?3..=space_ids
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        params_vec.push(Box::new(sanitized));
        params_vec.push(Box::new(limit as i64));
        for sid in space_ids {
            params_vec.push(Box::new(sid.clone()));
        }
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let results = stmt
            .query_map(param_refs.as_slice(), |row| {
                let id_str: String = row.get(0)?;
                let space_id: String = row.get(1)?;
                let score: f64 = row.get(2)?;
                Ok(FtsResult {
                    memory_id: Uuid::parse_str(&id_str).unwrap_or_default(),
                    bm25_score: score as f32,
                    space_id,
                })
            })
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("fts query: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("fts collect: {e}")))?;

        Ok(results)
    }

    async fn get_embedding(&self, id: Uuid) -> Result<Option<Vec<f32>>, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding FROM memories WHERE id = ?1 AND deleted_at IS NULL",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("get embedding: {e}")))?
            .flatten();

        Ok(blob.map(|b| bytes_to_f32s(&b)))
    }

    // ── Maintenance ──

    async fn decay_candidates(
        &self,
        space_ids: &[String],
        pinned_ids: &[Uuid],
        min_age_days: u32,
        importance_floor: f32,
        min_interval_hours: u32,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<DecayCandidate>, StoreError> {
        if space_ids.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().map_err(lock_err)?;
        let cutoff = Utc::now() - chrono::Duration::days(min_age_days as i64);
        let decay_cutoff = Utc::now() - chrono::Duration::hours(min_interval_hours as i64);

        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut param_idx = 1usize;

        let space_placeholders: String = space_ids
            .iter()
            .map(|sid| {
                let p = format!("?{param_idx}");
                params_vec.push(Box::new(sid.clone()));
                param_idx += 1;
                p
            })
            .collect::<Vec<_>>()
            .join(", ");

        let pinned_exclusion = if pinned_ids.is_empty() {
            String::new()
        } else {
            let placeholders: String = pinned_ids
                .iter()
                .map(|id| {
                    let p = format!("?{param_idx}");
                    params_vec.push(Box::new(id.to_string()));
                    param_idx += 1;
                    p
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("AND m.id NOT IN ({placeholders})")
        };

        params_vec.push(Box::new(cutoff.to_rfc3339()));
        let cutoff_param = format!("?{param_idx}");
        param_idx += 1;

        params_vec.push(Box::new(importance_floor as f64));
        let floor_param = format!("?{param_idx}");
        param_idx += 1;

        params_vec.push(Box::new(decay_cutoff.to_rfc3339()));
        let decay_cutoff_param = format!("?{param_idx}");
        param_idx += 1;

        params_vec.push(Box::new(limit as i64));
        let limit_param = format!("?{param_idx}");
        param_idx += 1;

        params_vec.push(Box::new(offset as i64));
        let offset_param = format!("?{param_idx}");

        // NULL last_decayed_at means never decayed through this path, which is the state
        // of every row predating the column -- those are eligible immediately.
        let sql = format!(
            "SELECT m.id, m.importance, m.created_at, m.space_id
             FROM memories m
             WHERE m.space_id IN ({space_placeholders})
               AND m.deleted_at IS NULL
               AND m.is_chunk = 0
               AND m.created_at < {cutoff_param}
               AND COALESCE(m.importance, 0.5) > {floor_param}
               AND (m.last_decayed_at IS NULL OR m.last_decayed_at < {decay_cutoff_param})
               {pinned_exclusion}
             ORDER BY m.created_at ASC
             LIMIT {limit_param} OFFSET {offset_param}"
        );

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("decay_candidates prepare: {e}")))?;

        let results = stmt
            .query_map(param_refs.as_slice(), |row| {
                let id_str: String = row.get(0)?;
                let importance: Option<f64> = row.get(1)?;
                let created_at_str: String = row.get(2)?;
                let space_id: String = row.get(3)?;
                Ok(DecayCandidate {
                    memory_id: Uuid::parse_str(&id_str).unwrap_or_default(),
                    importance: importance.unwrap_or(0.5) as f32,
                    created_at: parse_timestamp(&created_at_str),
                    space_id,
                })
            })
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("decay_candidates query: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("decay_candidates collect: {e}")))?;

        Ok(results)
    }

    async fn batch_update_importance(&self, updates: &[(Uuid, f32)]) -> Result<u32, StoreError> {
        if updates.is_empty() {
            return Ok(0);
        }

        let conn = self.conn.lock().map_err(lock_err)?;
        let mut count = 0u32;

        let now = Utc::now().to_rfc3339();
        for (id, new_importance) in updates {
            let affected = conn
                .execute(
                    "UPDATE memories
                        SET importance = ?1, last_decayed_at = ?3
                      WHERE id = ?2 AND deleted_at IS NULL",
                    params![*new_importance as f64, id.to_string(), now],
                )
                .map_err(|e| {
                    StoreError::Internal(anyhow::anyhow!("batch_update_importance: {e}"))
                })?;
            count += affected as u32;
        }

        Ok(count)
    }

    async fn prune_candidates(
        &self,
        space_ids: &[String],
        pinned_ids: &[Uuid],
        importance_threshold: f32,
        min_age_days: u32,
        limit: usize,
    ) -> Result<Vec<Uuid>, StoreError> {
        if space_ids.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().map_err(lock_err)?;
        let cutoff = Utc::now() - chrono::Duration::days(min_age_days as i64);

        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut param_idx = 1usize;

        let space_placeholders: String = space_ids
            .iter()
            .map(|sid| {
                let p = format!("?{param_idx}");
                params_vec.push(Box::new(sid.clone()));
                param_idx += 1;
                p
            })
            .collect::<Vec<_>>()
            .join(", ");

        let pinned_exclusion = if pinned_ids.is_empty() {
            String::new()
        } else {
            let placeholders: String = pinned_ids
                .iter()
                .map(|id| {
                    let p = format!("?{param_idx}");
                    params_vec.push(Box::new(id.to_string()));
                    param_idx += 1;
                    p
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("AND m.id NOT IN ({placeholders})")
        };

        params_vec.push(Box::new(importance_threshold as f64));
        let threshold_param = format!("?{param_idx}");
        param_idx += 1;

        params_vec.push(Box::new(cutoff.to_rfc3339()));
        let cutoff_param = format!("?{param_idx}");
        param_idx += 1;

        params_vec.push(Box::new(limit as i64));
        let limit_param = format!("?{param_idx}");

        let sql = format!(
            "SELECT m.id
             FROM memories m
             WHERE m.space_id IN ({space_placeholders})
               AND m.deleted_at IS NULL
               AND m.is_chunk = 0
               AND COALESCE(m.importance, 0.5) < {threshold_param}
               AND m.created_at < {cutoff_param}
               {pinned_exclusion}
             ORDER BY m.importance ASC, m.created_at ASC
             LIMIT {limit_param}"
        );

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("prune_candidates prepare: {e}")))?;

        let results = stmt
            .query_map(param_refs.as_slice(), |row| {
                let id_str: String = row.get(0)?;
                Ok(Uuid::parse_str(&id_str).unwrap_or_default())
            })
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("prune_candidates query: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("prune_candidates collect: {e}")))?;

        Ok(results)
    }

    async fn find_by_content_hash(
        &self,
        space_id: &str,
        hash: &[u8; 32],
    ) -> Result<Option<Uuid>, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        // Oldest wins — the duplicate should collapse onto the original, not the reverse,
        // so `created_at` and any provenance on the first write are preserved.
        let mut stmt = conn
            .prepare(
                "SELECT id FROM memories
                  WHERE space_id = ?1 AND content_hash = ?2
                    AND deleted_at IS NULL AND is_chunk = 0
                  ORDER BY created_at ASC
                  LIMIT 1",
            )
            .map_err(|e| {
                StoreError::Internal(anyhow::anyhow!("find_by_content_hash prepare: {e}"))
            })?;

        let found: Option<String> = stmt
            .query_row(params![space_id, hash.as_slice()], |row| row.get(0))
            .optional()
            .map_err(|e| {
                StoreError::Internal(anyhow::anyhow!("find_by_content_hash query: {e}"))
            })?;

        Ok(found.and_then(|s| Uuid::parse_str(&s).ok()))
    }

    async fn backup_to(&self, dest: &std::path::Path) -> Result<(), StoreError> {
        if dest.exists() {
            return Err(StoreError::Internal(anyhow::anyhow!(
                "backup destination already exists: {}",
                dest.display()
            )));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                StoreError::Internal(anyhow::anyhow!(
                    "create backup dir {}: {e}",
                    parent.display()
                ))
            })?;
        }

        let conn = self.conn.lock().map_err(lock_err)?;
        // VACUUM INTO writes a consistent snapshot including WAL contents, without
        // requiring exclusive access. Equivalent to sqlite3's `.backup`.
        conn.execute("VACUUM INTO ?1", params![dest.to_string_lossy()])
            .map_err(|e| {
                StoreError::Internal(anyhow::anyhow!("backup to {}: {e}", dest.display()))
            })?;

        Ok(())
    }

    async fn insert_forget_log(&self, entry: &ForgetLogEntry) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        ensure_forget_log_schema(&conn)?;

        conn.execute(
            "INSERT INTO forget_log (memory_id, content_hash, deleted_at, reason, actor)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                entry.memory_id.to_string(),
                entry.content_hash.as_slice(),
                entry.deleted_at.to_rfc3339(),
                entry.reason.as_str(),
                entry.actor.as_deref(),
            ],
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("insert_forget_log: {e}")))?;

        Ok(())
    }

    async fn list_forget_log(&self, limit: usize) -> Result<Vec<ForgetLogEntry>, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;
        ensure_forget_log_schema(&conn)?;

        let mut stmt = conn
            .prepare(
                "SELECT memory_id, content_hash, deleted_at, reason, actor
                 FROM forget_log
                 ORDER BY deleted_at DESC
                 LIMIT ?1",
            )
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("list_forget_log prepare: {e}")))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let id_str: String = row.get(0)?;
                let hash_blob: Vec<u8> = row.get(1)?;
                let deleted_at: String = row.get(2)?;
                let reason: String = row.get(3)?;
                let actor: Option<String> = row.get(4)?;
                Ok((id_str, hash_blob, deleted_at, reason, actor))
            })
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("list_forget_log query: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("list_forget_log collect: {e}")))?;

        rows.into_iter()
            .map(|(id_str, hash_blob, deleted_at, reason, actor)| {
                let memory_id = Uuid::parse_str(&id_str).map_err(|e| {
                    StoreError::Internal(anyhow::anyhow!("forget_log memory_id {id_str}: {e}"))
                })?;

                let mut content_hash = [0u8; 32];
                if hash_blob.len() != 32 {
                    return Err(StoreError::Internal(anyhow::anyhow!(
                        "forget_log content_hash for {memory_id} is {} bytes, expected 32",
                        hash_blob.len()
                    )));
                }
                content_hash.copy_from_slice(&hash_blob);

                // Deliberately loud on an unrecognised reason rather than skipping the row.
                // Dropping audit entries you cannot classify is how a log ends up quietly
                // understating what was deleted.
                let reason = ForgetReason::from_str(&reason).ok_or_else(|| {
                    StoreError::Internal(anyhow::anyhow!(
                        "forget_log has unknown reason {reason:?} for {memory_id}"
                    ))
                })?;

                Ok(ForgetLogEntry {
                    memory_id,
                    content_hash,
                    deleted_at: parse_timestamp(&deleted_at),
                    reason,
                    actor,
                })
            })
            .collect()
    }

    // ── Contradiction Detection ──

    // ── Nightly Run Tracking ──

    async fn last_nightly_run(
        &self,
        agent_id: &str,
    ) -> Result<Option<NightlyRunRecord>, StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS nightly_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ran_at TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                duration_ms INTEGER NOT NULL,
                summary TEXT NOT NULL
            )",
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("nightly_runs schema: {e}")))?;

        let result = conn
            .query_row(
                "SELECT ran_at, agent_id, duration_ms, summary
             FROM nightly_runs
             WHERE agent_id = ?1
             ORDER BY ran_at DESC
             LIMIT 1",
                params![agent_id],
                |row| {
                    let ran_at_str: String = row.get(0)?;
                    let agent_id: String = row.get(1)?;
                    let duration_ms: u64 = row.get(2)?;
                    let summary: String = row.get(3)?;
                    Ok(NightlyRunRecord {
                        ran_at: parse_timestamp(&ran_at_str),
                        agent_id,
                        duration_ms,
                        summary,
                    })
                },
            )
            .optional()
            .map_err(|e| StoreError::Internal(anyhow::anyhow!("last_nightly_run: {e}")))?;

        Ok(result)
    }

    async fn record_nightly_run(&self, record: &NightlyRunRecord) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(lock_err)?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS nightly_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ran_at TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                duration_ms INTEGER NOT NULL,
                summary TEXT NOT NULL
            )",
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("nightly_runs schema: {e}")))?;

        conn.execute(
            "INSERT INTO nightly_runs (ran_at, agent_id, duration_ms, summary)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                record.ran_at.to_rfc3339(),
                record.agent_id,
                record.duration_ms,
                record.summary,
            ],
        )
        .map_err(|e| StoreError::Internal(anyhow::anyhow!("record_nightly_run: {e}")))?;

        Ok(())
    }

    // ── Contradiction Detection ──
}

// ── Conversion Helpers ──────────────────────────────────────────────

fn row_to_memory(row: &rusqlite::Row) -> Memory {
    let id_str: String = row.get(0).unwrap_or_default();
    let content: String = row.get(1).unwrap_or_default();
    let embedding_blob: Option<Vec<u8>> = row.get(2).unwrap_or(None);
    let space_id: String = row.get(3).unwrap_or_default();

    let kind_str: String = row.get(4).unwrap_or_default();
    let source_str: String = row.get(5).unwrap_or_default();
    let tags_json: String = row.get(6).unwrap_or_else(|_| "[]".to_string());
    let created_at_str: String = row.get(7).unwrap_or_default();
    let content_hash_blob: Vec<u8> = row.get(8).unwrap_or_default();

    let importance: Option<f64> = row.get(9).unwrap_or(None);
    let confidence: Option<f64> = row.get(10).unwrap_or(None);
    let belief_str: Option<String> = row.get(11).unwrap_or(None);

    let parent_id_str: Option<String> = row.get(12).unwrap_or(None);
    let chunk_index: Option<i32> = row.get(13).unwrap_or(None);
    let is_chunk_int: i32 = row.get(14).unwrap_or(0);

    let source_agent: Option<String> = row.get(15).unwrap_or(None);
    let source_memory_str: Option<String> = row.get(16).unwrap_or(None);
    let source_space: Option<String> = row.get(17).unwrap_or(None);
    let shared_at_str: Option<String> = row.get(18).unwrap_or(None);

    let mut hash = [0u8; 32];
    if content_hash_blob.len() == 32 {
        hash.copy_from_slice(&content_hash_blob);
    }

    Memory {
        id: Uuid::parse_str(&id_str).unwrap_or_default(),
        content,
        embedding: embedding_blob.map(|b| bytes_to_f32s(&b)),
        space_id,
        kind: str_to_kind(&kind_str),
        source: str_to_source(&source_str),
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        created_at: parse_timestamp(&created_at_str),
        content_hash: hash,
        importance: importance.map(|v| v as f32),
        confidence: confidence.map(|v| v as f32),
        belief_state: belief_str.as_deref().map(str_to_belief),
        parent_id: parent_id_str.and_then(|s| Uuid::parse_str(&s).ok()),
        chunk_index: chunk_index.map(|i| i as u16),
        is_chunk: is_chunk_int != 0,
        source_agent,
        source_memory: source_memory_str.and_then(|s| Uuid::parse_str(&s).ok()),
        source_space,
        shared_at: shared_at_str.map(|s| parse_timestamp(&s)),
    }
}

fn kind_to_str(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Observation => "observation",
        MemoryKind::Fact => "fact",
        MemoryKind::Decision => "decision",
        MemoryKind::Preference => "preference",
        MemoryKind::Constraint => "constraint",
        MemoryKind::Procedure => "procedure",
    }
}

fn str_to_kind(s: &str) -> MemoryKind {
    match s {
        "fact" => MemoryKind::Fact,
        "decision" => MemoryKind::Decision,
        "preference" => MemoryKind::Preference,
        "constraint" => MemoryKind::Constraint,
        "procedure" => MemoryKind::Procedure,
        _ => MemoryKind::Observation,
    }
}

fn source_to_str(source: MemorySource) -> &'static str {
    match source {
        MemorySource::Agent => "agent",
        MemorySource::Human => "human",
        MemorySource::System => "system",
    }
}

fn str_to_source(s: &str) -> MemorySource {
    match s {
        "human" => MemorySource::Human,
        "system" => MemorySource::System,
        _ => MemorySource::Agent,
    }
}

fn belief_to_str(state: BeliefState) -> &'static str {
    match state {
        BeliefState::Active => "active",
        BeliefState::Superseded => "superseded",
        BeliefState::Uncertain => "uncertain",
    }
}

fn str_to_belief(s: &str) -> BeliefState {
    match s {
        "superseded" => BeliefState::Superseded,
        "uncertain" => BeliefState::Uncertain,
        _ => BeliefState::Active,
    }
}

/// Convert f32 slice to little-endian bytes for blob storage.
fn f32s_to_bytes(floats: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(floats.len() * 4);
    for f in floats {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

/// Convert little-endian bytes back to f32 vec.
fn bytes_to_f32s(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Create `forget_log` if absent and bring it up to the current shape.
///
/// The table is created lazily rather than in `init_schema`, so both the reader and the
/// writer have to guarantee it — this is the single place that does, instead of the two
/// drifting `CREATE TABLE IF NOT EXISTS` copies it replaces.
///
/// `actor` was added 2026-07-26. `ADD COLUMN` on an existing table is expected to fail
/// with "duplicate column name" on every call after the first, so the error is swallowed
/// rather than propagated: this runs on a path that must never make the store unopenable.
/// A genuinely broken table surfaces on the next read instead, which is the tradeoff
/// already taken for `idx_memories_content_hash_live`.
fn ensure_forget_log_schema(conn: &rusqlite::Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS forget_log (
            memory_id    TEXT NOT NULL,
            content_hash BLOB NOT NULL,
            deleted_at   TEXT NOT NULL,
            reason       TEXT NOT NULL,
            actor        TEXT
        )",
    )
    .map_err(|e| StoreError::Internal(anyhow::anyhow!("forget_log schema: {e}")))?;

    let _ = conn.execute("ALTER TABLE forget_log ADD COLUMN actor TEXT", []);

    Ok(())
}

fn parse_timestamp(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_default()
}

fn lock_err<T>(_: T) -> StoreError {
    StoreError::Internal(anyhow::anyhow!("mutex lock poisoned"))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> SqliteStore {
        SqliteStore::in_memory().expect("failed to create in-memory store")
    }

    fn test_space() -> Space {
        Space {
            id: "test-space".to_string(),
            rotation_kind: RotationKind::Identity,
            rotation_seed: None,
            created_at: Utc::now(),
            memory_count: 0,
        }
    }

    /// A distinct memory each call.
    ///
    /// Content is keyed to the id deliberately. It used to be the fixed string
    /// "Test memory content", so every fixture memory shared one `content_hash` — which the
    /// live unique index on (space_id, content_hash) now rejects. Several tests inserted three
    /// of these to assert "3 memories exist", so they were depending on duplicate rows being
    /// legal. They aren't.
    fn test_memory(space_id: &str) -> Memory {
        let id = Uuid::new_v4();
        let content = format!("Test memory content {id}");
        Memory {
            id,
            content: content.clone(),
            embedding: Some(vec![1.0, 2.0, 3.0]),
            space_id: space_id.to_string(),
            kind: MemoryKind::Observation,
            source: MemorySource::Agent,
            tags: vec!["test".to_string()],
            created_at: Utc::now(),
            content_hash: crate::model::content_hash(&content),
            importance: Some(0.8),
            confidence: None,
            belief_state: Some(BeliefState::Active),
            parent_id: None,
            chunk_index: None,
            is_chunk: false,
            source_agent: Some("demo".to_string()),
            source_memory: None,
            source_space: None,
            shared_at: None,
        }
    }

    #[tokio::test]
    async fn insert_and_get_memory() {
        let store = test_store();
        let space = test_space();
        store.create_space(&space).await.unwrap();

        let mem = test_memory(&space.id);
        let id = mem.id;
        store.insert_memory(&mem).await.unwrap();

        let got = store.get_memory(id).await.unwrap();
        assert_eq!(got.id, id);
        assert!(got.content.starts_with("Test memory content "));
        assert_eq!(got.kind, MemoryKind::Observation);
        assert_eq!(got.source, MemorySource::Agent);
        assert_eq!(got.tags, vec!["test"]);
        assert_eq!(got.importance, Some(0.8));
        assert_eq!(got.belief_state, Some(BeliefState::Active));
        assert!(got.embedding.is_some());
        assert_eq!(got.embedding.unwrap(), vec![1.0, 2.0, 3.0]);
    }

    #[tokio::test]
    async fn delete_memory_soft() {
        let store = test_store();
        let space = test_space();
        store.create_space(&space).await.unwrap();

        let mem = test_memory(&space.id);
        let id = mem.id;
        store.insert_memory(&mem).await.unwrap();
        store.delete_memory(id).await.unwrap();

        // Should not be findable
        assert!(store.get_memory(id).await.is_err());

        // Should show up as tombstone
        let tombstones = store.count_tombstones().await.unwrap();
        assert_eq!(tombstones, 1);
    }

    #[tokio::test]
    async fn parent_chunk_lifecycle() {
        let store = test_store();
        let space = test_space();
        store.create_space(&space).await.unwrap();

        let parent_id = Uuid::new_v4();
        let parent = Memory {
            id: parent_id,
            content: "Full parent content".to_string(),
            embedding: None,
            space_id: space.id.clone(),
            kind: MemoryKind::Fact,
            source: MemorySource::Agent,
            tags: vec![],
            created_at: Utc::now(),
            content_hash: crate::model::content_hash("Full parent content"),
            importance: None,
            confidence: None,
            belief_state: Some(BeliefState::Active),
            parent_id: None,
            chunk_index: None,
            is_chunk: false,
            source_agent: None,
            source_memory: None,
            source_space: None,
            shared_at: None,
        };
        store.insert_memory(&parent).await.unwrap();

        // Add two chunks
        for i in 0..2 {
            let chunk = Memory {
                id: Uuid::new_v4(),
                content: format!("Chunk {i}"),
                embedding: Some(vec![i as f32; 3]),
                space_id: space.id.clone(),
                kind: MemoryKind::Fact,
                source: MemorySource::Agent,
                tags: vec![],
                created_at: Utc::now(),
                content_hash: crate::model::content_hash(&format!("Chunk {i}")),
                importance: None,
                confidence: None,
                belief_state: Some(BeliefState::Active),
                parent_id: Some(parent_id),
                chunk_index: Some(i),
                is_chunk: true,
                source_agent: None,
                source_memory: None,
                source_space: None,
                shared_at: None,
            };
            store.insert_memory(&chunk).await.unwrap();
        }

        assert!(store.is_parent(parent_id).await.unwrap());
        assert!(!store.is_chunk(parent_id).await.unwrap());

        let chunks = store.get_chunks(parent_id).await.unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chunk_index, Some(0));
        assert_eq!(chunks[1].chunk_index, Some(1));

        // Delete parent cascade
        let deleted = store.delete_parent_and_chunks(parent_id).await.unwrap();
        assert_eq!(deleted, 3); // parent + 2 chunks
    }

    #[tokio::test]
    async fn belief_state_update() {
        let store = test_store();
        let space = test_space();
        store.create_space(&space).await.unwrap();

        let mem = test_memory(&space.id);
        let id = mem.id;
        store.insert_memory(&mem).await.unwrap();

        store
            .update_belief_state(&[id], BeliefState::Superseded)
            .await
            .unwrap();

        let got = store.get_memory(id).await.unwrap();
        assert_eq!(got.belief_state, Some(BeliefState::Superseded));
    }

    #[tokio::test]
    async fn space_and_keyring() {
        let store = test_store();
        let space = test_space();
        store.create_space(&space).await.unwrap();

        let got = store.get_space("test-space").await.unwrap();
        assert_eq!(got.id, "test-space");
        assert_eq!(got.rotation_kind, RotationKind::Identity);

        let entry = KeyRingEntry {
            agent_id: "demo".to_string(),
            space_id: "test-space".to_string(),
            granted_at: Utc::now(),
            granted_by: "operator".to_string(),
        };
        store.grant_access(&entry).await.unwrap();

        let ring = store.get_key_ring("demo").await.unwrap();
        assert_eq!(ring.len(), 1);
        assert_eq!(ring[0].space_id, "test-space");

        store.revoke_access("demo", "test-space").await.unwrap();
        let ring = store.get_key_ring("demo").await.unwrap();
        assert_eq!(ring.len(), 0);
    }

    #[tokio::test]
    async fn all_vectors_in_space() {
        let store = test_store();
        let space = test_space();
        store.create_space(&space).await.unwrap();

        // Insert 3 memories with embeddings
        for _ in 0..3 {
            let mem = test_memory(&space.id);
            store.insert_memory(&mem).await.unwrap();
        }

        let vectors = store.all_vectors_in_space(&space.id).await.unwrap();
        assert_eq!(vectors.len(), 3);
        assert_eq!(vectors[0].1, vec![1.0, 2.0, 3.0]);
    }

    #[tokio::test]
    async fn count_memories_across_spaces() {
        let store = test_store();

        let space1 = Space {
            id: "space-a".to_string(),
            ..test_space()
        };
        let space2 = Space {
            id: "space-b".to_string(),
            ..test_space()
        };
        store.create_space(&space1).await.unwrap();
        store.create_space(&space2).await.unwrap();

        for _ in 0..3 {
            store.insert_memory(&test_memory("space-a")).await.unwrap();
        }
        for _ in 0..2 {
            store.insert_memory(&test_memory("space-b")).await.unwrap();
        }

        assert_eq!(store.count_memories("space-a").await.unwrap(), 3);
        assert_eq!(store.count_memories("space-b").await.unwrap(), 2);
    }

    #[tokio::test]
    async fn pin_unpin_lifecycle() {
        let store = test_store();
        let space = test_space();
        store.create_space(&space).await.unwrap();

        let mem = test_memory(&space.id);
        let id = mem.id;
        store.insert_memory(&mem).await.unwrap();

        // Not pinned initially
        assert!(!store.is_pinned("demo", id).await.unwrap());
        assert_eq!(store.get_pinned("demo").await.unwrap().len(), 0);

        // Pin it
        store.pin_memory("demo", id).await.unwrap();
        assert!(store.is_pinned("demo", id).await.unwrap());
        assert_eq!(store.get_pinned("demo").await.unwrap(), vec![id]);

        // Pin is idempotent (INSERT OR IGNORE)
        store.pin_memory("demo", id).await.unwrap();
        assert_eq!(store.get_pinned("demo").await.unwrap().len(), 1);

        // Unpin
        store.unpin_memory("demo", id).await.unwrap();
        assert!(!store.is_pinned("demo", id).await.unwrap());
        assert_eq!(store.get_pinned("demo").await.unwrap().len(), 0);

        // Unpin non-pinned is a no-op
        store.unpin_memory("demo", id).await.unwrap();
    }

    #[tokio::test]
    async fn pin_isolation_between_agents() {
        let store = test_store();
        let space = test_space();
        store.create_space(&space).await.unwrap();

        let mem = test_memory(&space.id);
        let id = mem.id;
        store.insert_memory(&mem).await.unwrap();

        store.pin_memory("demo", id).await.unwrap();
        assert!(store.is_pinned("demo", id).await.unwrap());
        assert!(!store.is_pinned("echo", id).await.unwrap());
    }

    #[tokio::test]
    async fn filter_by_kind() {
        let store = test_store();
        let space = test_space();
        store.create_space(&space).await.unwrap();

        // Insert observation and fact
        let mut obs = test_memory(&space.id);
        obs.kind = MemoryKind::Observation;
        store.insert_memory(&obs).await.unwrap();

        let mut fact = test_memory(&space.id);
        fact.kind = MemoryKind::Fact;
        fact.content = "A known fact".to_string();
        store.insert_memory(&fact).await.unwrap();

        let filter = FilterInput {
            kind: Some(MemoryKind::Fact),
            ..Default::default()
        };
        let (results, total) = store
            .filter_memories(&[space.id.clone()], &filter, 20, 0)
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, MemoryKind::Fact);
    }

    #[tokio::test]
    async fn filter_excludes_chunks() {
        let store = test_store();
        let space = test_space();
        store.create_space(&space).await.unwrap();

        // Insert parent + chunk
        let parent_id = Uuid::new_v4();
        let parent = Memory {
            id: parent_id,
            content: "Parent content".to_string(),
            embedding: None,
            space_id: space.id.clone(),
            kind: MemoryKind::Fact,
            source: MemorySource::Agent,
            tags: vec![],
            created_at: Utc::now(),
            content_hash: crate::model::content_hash("Parent content"),
            importance: None,
            confidence: None,
            belief_state: None,
            parent_id: None,
            chunk_index: None,
            is_chunk: false,
            source_agent: None,
            source_memory: None,
            source_space: None,
            shared_at: None,
        };
        store.insert_memory(&parent).await.unwrap();

        let chunk = Memory {
            id: Uuid::new_v4(),
            content: "Chunk 0".to_string(),
            embedding: Some(vec![1.0, 2.0, 3.0]),
            is_chunk: true,
            parent_id: Some(parent_id),
            chunk_index: Some(0),
            ..parent.clone()
        };
        store.insert_memory(&chunk).await.unwrap();

        let filter = FilterInput::default();
        let (results, total) = store
            .filter_memories(&[space.id.clone()], &filter, 20, 0)
            .await
            .unwrap();
        // Only the parent should appear, not the chunk
        assert_eq!(total, 1);
        assert_eq!(results[0].memory_id, parent_id);
    }

    #[tokio::test]
    async fn filter_by_pinned() {
        let store = test_store();
        let space = test_space();
        store.create_space(&space).await.unwrap();

        let mem1 = test_memory(&space.id);
        let mem2 = test_memory(&space.id);
        store.insert_memory(&mem1).await.unwrap();
        store.insert_memory(&mem2).await.unwrap();

        store.pin_memory("demo", mem1.id).await.unwrap();

        // Filter for pinned only
        let filter = FilterInput {
            pinned: Some(true),
            ..Default::default()
        };
        let (results, total) = store
            .filter_memories(&[space.id.clone()], &filter, 20, 0)
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(results[0].memory_id, mem1.id);
        assert!(results[0].pinned);

        // Filter for unpinned only
        let filter = FilterInput {
            pinned: Some(false),
            ..Default::default()
        };
        let (results, total) = store
            .filter_memories(&[space.id.clone()], &filter, 20, 0)
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(results[0].memory_id, mem2.id);
        assert!(!results[0].pinned);
    }

    #[tokio::test]
    async fn filter_pagination() {
        let store = test_store();
        let space = test_space();
        store.create_space(&space).await.unwrap();

        // Insert 5 memories
        for _ in 0..5 {
            store.insert_memory(&test_memory(&space.id)).await.unwrap();
        }

        let filter = FilterInput::default();

        // Page 1: limit 2, offset 0
        let (results, total) = store
            .filter_memories(&[space.id.clone()], &filter, 2, 0)
            .await
            .unwrap();
        assert_eq!(total, 5);
        assert_eq!(results.len(), 2);

        // Page 2: limit 2, offset 2
        let (results, _) = store
            .filter_memories(&[space.id.clone()], &filter, 2, 2)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);

        // Page 3: limit 2, offset 4
        let (results, _) = store
            .filter_memories(&[space.id.clone()], &filter, 2, 4)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn f32_blob_roundtrip() {
        let original = vec![1.0f32, -2.5, 3.14159, 0.0, f32::MAX, f32::MIN];
        let bytes = f32s_to_bytes(&original);
        let recovered = bytes_to_f32s(&bytes);
        assert_eq!(original, recovered);
    }

    // ── superseded_at migration ─────────────────────────────────────
    //
    // Every other test builds the schema fresh, so the column is already there. These
    // exercise the path that actually runs against an existing database.

    fn has_superseded_at(conn: &rusqlite::Connection) -> bool {
        conn.prepare("SELECT 1 FROM pragma_table_info('memories') WHERE name = 'superseded_at'")
            .and_then(|mut s| s.exists([]))
            .unwrap()
    }

    /// Build a connection whose `memories` table predates `superseded_at`.
    fn pre_migration_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        conn.execute_batch("ALTER TABLE memories DROP COLUMN superseded_at")
            .unwrap();
        assert!(
            !has_superseded_at(&conn),
            "fixture should start without the column"
        );
        conn.execute_batch(
            "INSERT INTO spaces (id, rotation_kind, rotation_seed, created_at, memory_count)
             VALUES ('agent:demo', 'identity', NULL, '2026-01-01T00:00:00+00:00', 0)",
        )
        .unwrap();
        conn
    }

    fn insert_memory(conn: &rusqlite::Connection, id: &str, belief: &str, created_at: &str) {
        conn.execute(
            "INSERT INTO memories (id, content, space_id, kind, source, tags, created_at,
                                   content_hash, importance, belief_state, is_chunk)
             VALUES (?1, 'x', 'agent:demo', 'fact', 'agent', '[]', ?3, X'00', 0.7, ?2, 0)",
            params![id, belief, created_at],
        )
        .unwrap();
    }

    #[test]
    fn migration_adds_column_and_backfills_superseded_rows() {
        let conn = pre_migration_conn();
        let old = "11111111-1111-1111-1111-111111111111";
        let active = "22222222-2222-2222-2222-222222222222";
        // Deliberately ancient: under the old predicate this row was deletable immediately.
        insert_memory(&conn, old, "superseded", "2026-01-01T00:00:00+00:00");
        insert_memory(&conn, active, "active", "2026-01-01T00:00:00+00:00");

        SqliteStore::migrate_superseded_at(&conn).unwrap();

        assert!(
            has_superseded_at(&conn),
            "column should exist after migration"
        );

        let stamped: Option<String> = conn
            .query_row(
                "SELECT superseded_at FROM memories WHERE id = ?1",
                params![old],
                |r| r.get(0),
            )
            .unwrap();
        let stamped = stamped.expect("superseded row must be backfilled, not left NULL");

        // Backfilled to migration time, NOT created_at -- using created_at is the bug.
        assert!(
            !stamped.starts_with("2026-01-01"),
            "must not backfill from created_at, got {stamped}"
        );
        let age = Utc::now() - parse_timestamp(&stamped);
        assert!(
            age.num_seconds().abs() < 60,
            "backfill should stamp ~now, got {stamped}"
        );

        let untouched: Option<String> = conn
            .query_row(
                "SELECT superseded_at FROM memories WHERE id = ?1",
                params![active],
                |r| r.get(0),
            )
            .unwrap();
        assert!(untouched.is_none(), "active rows must stay NULL");
    }

    #[test]
    fn migration_is_idempotent_and_does_not_restamp() {
        let conn = pre_migration_conn();
        let id = "33333333-3333-3333-3333-333333333333";
        insert_memory(&conn, id, "superseded", "2026-01-01T00:00:00+00:00");

        SqliteStore::migrate_superseded_at(&conn).unwrap();
        let first: String = conn
            .query_row(
                "SELECT superseded_at FROM memories WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();

        // A second run must not error, and must not push the retention clock forward.
        SqliteStore::migrate_superseded_at(&conn).unwrap();
        let second: String = conn
            .query_row(
                "SELECT superseded_at FROM memories WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(
            first, second,
            "re-running the migration must not restamp rows"
        );
    }

    #[test]
    fn migration_runs_automatically_on_open() {
        // init_schema must invoke the migration -- a store opened on a pre-migration
        // database has to come up usable.
        let store = test_store();
        let conn = store.conn.lock().unwrap();
        assert!(has_superseded_at(&conn));
    }
}

#[cfg(test)]
mod content_hash_index_tests {
    use super::*;

    fn index_exists(conn: &rusqlite::Connection) -> bool {
        conn.prepare(
            "SELECT 1 FROM sqlite_master
              WHERE type='index' AND name='idx_memories_content_hash_live'",
        )
        .and_then(|mut s| s.exists([]))
        .unwrap()
    }

    fn seed_space(conn: &rusqlite::Connection) {
        conn.execute_batch(
            "INSERT INTO spaces (id, rotation_kind, rotation_seed, created_at, memory_count)
             VALUES ('agent:demo','identity',NULL,'2026-01-01T00:00:00+00:00',0)",
        )
        .unwrap();
    }

    fn insert(
        conn: &rusqlite::Connection,
        id: &str,
        hash: &[u8],
        deleted: Option<&str>,
        chunk: i64,
    ) {
        conn.execute(
            "INSERT INTO memories (id, content, space_id, kind, source, tags, created_at,
                                   content_hash, importance, belief_state, is_chunk, deleted_at)
             VALUES (?1,'x','agent:demo','fact','agent','[]','2026-01-01T00:00:00+00:00',
                     ?2, 0.7,'active', ?3, ?4)",
            params![id, hash, chunk, deleted],
        )
        .unwrap();
    }

    #[test]
    fn index_is_created_on_a_clean_store() {
        let store = SqliteStore::in_memory().unwrap();
        let conn = store.conn.lock().unwrap();
        assert!(
            index_exists(&conn),
            "index should install on a clean database"
        );
    }

    /// The important one: a store with live duplicates must still OPEN. `init_schema` runs on
    /// every open, so a hard failure here would make the database unopenable — escalating a
    /// cosmetic data problem into total unavailability.
    #[test]
    fn duplicates_skip_the_index_instead_of_failing_the_open() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        conn.execute_batch("DROP INDEX IF EXISTS idx_memories_content_hash_live")
            .unwrap();
        seed_space(&conn);
        insert(
            &conn,
            "11111111-1111-1111-1111-111111111111",
            b"samehash",
            None,
            0,
        );
        insert(
            &conn,
            "22222222-2222-2222-2222-222222222222",
            b"samehash",
            None,
            0,
        );

        SqliteStore::migrate_content_hash_unique(&conn).expect("must not error on duplicates");
        assert!(
            !index_exists(&conn),
            "index must be skipped while duplicates are live"
        );

        // Collapse the duplicate the way `forget` does — soft delete — and reopen.
        conn.execute(
            "UPDATE memories SET deleted_at = '2026-07-25T00:00:00+00:00'
              WHERE id = '22222222-2222-2222-2222-222222222222'",
            [],
        )
        .unwrap();
        SqliteStore::migrate_content_hash_unique(&conn).unwrap();
        assert!(
            index_exists(&conn),
            "soft-deleted duplicates must not block the index"
        );
    }

    #[test]
    fn chunks_are_exempt_from_hash_uniqueness() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        seed_space(&conn);
        // Two documents can legitimately contain an identical fragment.
        insert(
            &conn,
            "33333333-3333-3333-3333-333333333333",
            b"fragment",
            None,
            1,
        );
        insert(
            &conn,
            "44444444-4444-4444-4444-444444444444",
            b"fragment",
            None,
            1,
        );

        SqliteStore::migrate_content_hash_unique(&conn)
            .expect("duplicate chunk hashes must not block the index");
        assert!(index_exists(&conn));
    }
}
