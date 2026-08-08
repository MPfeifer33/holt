//! Session memory — continuous background conversation summarization.
//!
//! At each engine loop turn boundary, checks trigger conditions. When met,
//! spawns a silent fork that writes a structured summary to disk. On compaction,
//! the summary is injected as a boot message so the agent never loses continuity.

use std::future::Future;
use std::path::PathBuf;

/// Runtime state for session memory extraction (per agent, not serialized).
#[derive(Debug, Clone, Default)]
pub struct SessionMemoryState {
    /// Token count when last extraction completed
    pub tokens_at_last_extraction: u64,
    /// Tool calls since last extraction
    pub tool_calls_since_extraction: u32,
    /// Assistant messages since last extraction
    pub assistant_messages_since_extraction: u32,
    /// Whether an extraction fork is currently running
    pub extraction_in_progress: bool,
    /// Whether the first extraction has occurred this session
    pub initialized: bool,
}

/// Configuration for session memory extraction thresholds.
pub struct SessionMemoryConfig {
    /// Minimum tokens before first extraction (default: 10,000)
    pub minimum_tokens_to_init: u64,
    /// Minimum token growth between extractions (default: 5,000)
    pub minimum_tokens_between_update: u64,
    /// Minimum tool calls between extractions (default: 3) — OR condition with messages
    pub tool_calls_between_updates: u32,
    /// Minimum assistant messages between extractions (default: 8) — OR condition with tool calls
    pub assistant_messages_between_updates: u32,
    /// Optional model override for extraction fork.
    pub model_override: Option<String>,
}

impl Default for SessionMemoryConfig {
    fn default() -> Self {
        Self {
            minimum_tokens_to_init: 10_000,
            minimum_tokens_between_update: 5_000,
            tool_calls_between_updates: 3,
            assistant_messages_between_updates: 8,
            model_override: None,
        }
    }
}

impl SessionMemoryState {
    /// Check whether all trigger conditions are met for extraction.
    ///
    /// Conditions (ALL must be true):
    /// 1. No extraction currently in progress
    /// 2. Context >= init threshold (first extraction) OR growth >= update threshold
    /// 3. Activity threshold: >= N tool calls OR >= M assistant messages
    pub fn should_trigger(&self, config: &SessionMemoryConfig, current_tokens: u64) -> bool {
        if self.extraction_in_progress {
            return false;
        }

        let token_ok = if !self.initialized {
            current_tokens >= config.minimum_tokens_to_init
        } else {
            current_tokens.saturating_sub(self.tokens_at_last_extraction)
                >= config.minimum_tokens_between_update
        };

        if !token_ok {
            return false;
        }

        self.tool_calls_since_extraction >= config.tool_calls_between_updates
            || self.assistant_messages_since_extraction >= config.assistant_messages_between_updates
    }

    /// Reset counters after a successful extraction.
    pub fn mark_extraction_complete(&mut self, current_tokens: u64) {
        self.tokens_at_last_extraction = current_tokens;
        self.tool_calls_since_extraction = 0;
        self.assistant_messages_since_extraction = 0;
        self.extraction_in_progress = false;
        self.initialized = true;
    }

    /// Reset state for a new session (called by clear_conversation).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

use std::sync::Arc;

use tauri::Emitter;

use crate::config::app_config::agent_dir;
use crate::memory::store::{MemoryCategory, MemorySource, MemoryTier, StoreInput};
use crate::runtime::agent::{AgentSlot, MessageRole};
use crate::runtime::connection::{AgentProtocol, ConnectionConfig};

/// Context extracted from a parent agent for session memory extraction.
struct SessionForkContext {
    pub system_prompt: String,
    pub context_messages: Vec<crate::runtime::agent::ConversationMessage>,
    pub connection: ConnectionConfig,
}

/// Build fork context from parent's cached params (preferred) or live state (fallback).
/// Strips trailing user messages from inherited context — those are the parent's
/// active task, not background context for the extraction.
fn build_fork_context(agent: &AgentSlot) -> SessionForkContext {
    // Clone out of Arc into owned Vec
    let raw_messages = if let Some(ref cached) = agent.cached_params {
        (*cached.context_messages).clone()
    } else {
        agent.conversation.messages.clone()
    };

    // Strip trailing user messages — the parent's active task shouldn't
    // pollute the extraction context.
    let mut messages = raw_messages;
    while messages.last().is_some_and(|m| m.role == MessageRole::User) {
        messages.pop();
    }

    // Always use the raw system prompt, NOT cached_params.system_prompt.
    // cached_params captures the fully augmented prompt (persona, memory,
    // cache sentinels, A2A messages, etc.). Using it as the fork's base
    // causes double-augmentation.
    let system_prompt = agent.system_prompt.clone();

    SessionForkContext {
        system_prompt,
        context_messages: messages,
        connection: agent.connection.clone(),
    }
}
use crate::runtime::session_memory_lifecycle::{
    write_session_memory_lifecycle_trace, SessionMemoryLifecycleReport, SessionMemoryStoreReceipt,
};

/// Required section headers in the session memory file.
pub const REQUIRED_HEADERS: &[&str] = &[
    "# Session Title",
    "# Current State",
    "# Task Specification",
    "# Files and Functions",
    "# Workflow",
    "# Errors & Corrections",
    "# Codebase and System Documentation",
    "# Learnings",
    "# Key Results",
    "# Worklog",
];

/// Path to the session memory file for a given agent and session.
pub fn session_memory_path(agent_id: &str, session_id: &str) -> PathBuf {
    agent_dir(agent_id)
        .join("memory")
        .join("sessions")
        .join(format!("{}.md", session_id))
}

/// The initial template for a new session memory file.
pub fn session_memory_template() -> String {
    r#"# Session Title
_A short and distinctive 5-10 word descriptive title for the session. Super info dense, no filler_

# Current State
_What is actively being worked on right now? Pending tasks not yet completed. Immediate next steps._

# Task Specification
_What did the user ask to build? Any design decisions or other explanatory context_

# Files and Functions
_What are the important files? In short, what do they contain and why are they relevant?_

# Workflow
_What bash commands are usually run and in what order? How to interpret their output if not obvious?_

# Errors & Corrections
_Errors encountered and how they were fixed. What did the user correct? What approaches failed and should not be tried again?_

# Codebase and System Documentation
_What are the important system components? How do they work/fit together?_

# Learnings
_What has worked well? What has not? What to avoid? Do not duplicate items from other sections_

# Key Results
_If the user asked a specific output such as an answer to a question, a table, or other document, repeat the exact result here_

# Worklog
_Step by step, what was attempted, done? Very terse summary for each step_
"#
    .to_string()
}

/// Validate that a session memory file still contains all required section headers.
pub fn validate_session_file(content: &str) -> bool {
    REQUIRED_HEADERS
        .iter()
        .all(|header| content.contains(header))
}

/// Ensure the session memory template file exists. Creates it (and parent dirs) if missing.
pub fn ensure_template_exists(path: &PathBuf) {
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, session_memory_template());
}

/// Create a backup of the session file before extraction.
pub fn backup_session_file(path: &PathBuf) -> std::io::Result<PathBuf> {
    let backup = path.with_extension("md.bak");
    std::fs::copy(path, &backup)?;
    Ok(backup)
}

/// Restore session file from backup.
pub fn restore_from_backup(path: &PathBuf, backup: &PathBuf) -> std::io::Result<()> {
    std::fs::copy(backup, path)?;
    std::fs::remove_file(backup)?;
    Ok(())
}

/// Build the extraction prompt with current session file content.
pub fn build_extraction_prompt(path: &str, current_content: &str) -> String {
    format!(
        r#"You are a session memory extractor. Your task is to update the session notes
file based on the conversation above.

Current session notes at {path}:
<current_notes>
{current_content}
</current_notes>

RULES:
- Use edit_file to update sections in {path}
- NEVER modify lines starting with # or _ (headers and descriptions are structural)
- Write Session Title on the FIRST extraction only. Leave it unchanged after.
- Write for your future self after compaction: what do you need to resume?
- Capture: file paths, function names, commands run, decisions + one-line reasoning,
  error messages, who asked for what
- Skip: greetings, abandoned approaches (unless instructive), anything already in
  the notes, casual conversation
- Current State is your boot message — write it as "You were doing X, last thing
  was Y, next step is Z"
- Worklog: append new entries chronologically. Max 30 entries. When exceeding,
  merge the oldest 5 entries into a single summary line. Preserve temporal
  ordering (earliest first).
- When updating existing content: merge, don't duplicate. Replace stale info,
  keep what's still true
- Each section: ~40-60 lines max. Quality over quantity.
- If no new information for a section, leave it untouched"#
    )
}

pub fn build_codex_extraction_prompt(path: &str, current_content: &str) -> String {
    format!(
        r#"You are a session memory extractor. Your task is to update the session notes
document based on the conversation above.

Current session notes that will be replaced at {path}:
<current_notes>
{current_content}
</current_notes>

RULES:
- Return the COMPLETE updated markdown document only
- Do NOT call tools
- Do NOT wrap the response in code fences
- Preserve every required section header exactly as written
- NEVER modify lines starting with _ (the italic structural descriptions)
- Write Session Title on the FIRST extraction only. Leave it unchanged after.
- Write for your future self after compaction: what do you need to resume?
- Capture: file paths, function names, commands run, decisions + one-line reasoning,
  error messages, who asked for what
- Skip: greetings, abandoned approaches (unless instructive), anything already in
  the notes, casual conversation
- Current State is your boot message — write it as "You were doing X, last thing
  was Y, next step is Z"
- Worklog: append new entries chronologically. Max 30 entries. When exceeding,
  merge the oldest 5 entries into a single summary line. Preserve temporal
  ordering (earliest first).
- When updating existing content: merge, don't duplicate. Replace stale info,
  keep what's still true
- Each section: ~40-60 lines max. Quality over quantity.
- If no new information for a section, leave it untouched"#,
    )
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut iter = input.chars();
    let truncated: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn compact_inline(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn bullet_list(items: &[String], fallback: &str) -> String {
    if items.is_empty() {
        return format!("- {fallback}");
    }
    items
        .iter()
        .map(|item| format!("- {}", item))
        .collect::<Vec<_>>()
        .join("\n")
}

fn recent_user_requests(agent: &AgentSlot, limit: usize) -> Vec<String> {
    agent
        .conversation
        .messages
        .iter()
        .rev()
        .filter(|message| message.role == MessageRole::User)
        .take(limit)
        .map(|message| truncate_chars(&compact_inline(&message.content), 220))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn recent_assistant_messages(agent: &AgentSlot, limit: usize) -> Vec<String> {
    agent
        .conversation
        .messages
        .iter()
        .rev()
        .filter(|message| message.role == MessageRole::Assistant)
        .take(limit)
        .map(|message| truncate_chars(&compact_inline(&message.content), 220))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn recent_tool_commands(agent: &AgentSlot, limit: usize) -> Vec<String> {
    let mut commands = Vec::new();
    for message in agent.conversation.messages.iter().rev() {
        if message.role != MessageRole::Tool {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&message.content) {
            if let Some(command) = value.get("command").and_then(|v| v.as_str()) {
                commands.push(truncate_chars(&compact_inline(command), 180));
            }
        }
        if commands.len() >= limit {
            break;
        }
    }
    commands.reverse();
    commands
}

fn recent_file_hints(agent: &AgentSlot, limit: usize) -> Vec<String> {
    let mut hints = Vec::new();
    for message in agent.conversation.messages.iter().rev() {
        let content = message.content.as_str();
        for token in content.split_whitespace() {
            let cleaned = token.trim_matches(|c: char| {
                matches!(
                    c,
                    '`' | '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';' | ':'
                )
            });
            let looks_like_path = cleaned.contains('/')
                || cleaned.contains('\\')
                || cleaned.ends_with(".rs")
                || cleaned.ends_with(".ts")
                || cleaned.ends_with(".tsx")
                || cleaned.ends_with(".js")
                || cleaned.ends_with(".svelte")
                || cleaned.ends_with(".md")
                || cleaned.ends_with(".toml")
                || cleaned.ends_with(".json");
            if looks_like_path && !cleaned.is_empty() {
                let cleaned = truncate_chars(cleaned, 180);
                if !hints.contains(&cleaned) {
                    hints.push(cleaned);
                }
                if hints.len() >= limit {
                    return hints;
                }
            }
        }
    }
    hints
}

fn recent_error_hints(agent: &AgentSlot, limit: usize) -> Vec<String> {
    let mut hints = Vec::new();
    for message in agent.conversation.messages.iter().rev() {
        let normalized = message.content.to_lowercase();
        let interesting = normalized.contains("error")
            || normalized.contains("failed")
            || normalized.contains("warning")
            || normalized.contains("fix")
            || normalized.contains("panic");
        if interesting {
            hints.push(truncate_chars(&compact_inline(&message.content), 220));
        }
        if hints.len() >= limit {
            break;
        }
    }
    hints.reverse();
    hints
}

fn recent_worklog(agent: &AgentSlot, limit: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for message in agent.conversation.messages.iter().rev() {
        let role = match message.role {
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
            MessageRole::Tool => "Tool",
            MessageRole::System => "System",
        };
        let line = format!(
            "{role}: {}",
            truncate_chars(&compact_inline(&message.content), 180)
        );
        lines.push(line);
        if lines.len() >= limit {
            break;
        }
    }
    lines.reverse();
    lines
}

fn sdk_session_title(agent: &AgentSlot, session_id: &str) -> String {
    recent_user_requests(agent, 1)
        .into_iter()
        .next()
        .map(|request| truncate_chars(&request, 72))
        .filter(|request| !request.trim().is_empty())
        .unwrap_or_else(|| format!("SDK session {}", truncate_chars(session_id, 12)))
}

fn build_sdk_session_memory_snapshot(agent: &AgentSlot, session_id: &str) -> String {
    let user_requests = recent_user_requests(agent, 3);
    let assistant_messages = recent_assistant_messages(agent, 2);
    let commands = recent_tool_commands(agent, 8);
    let file_hints = recent_file_hints(agent, 8);
    let error_hints = recent_error_hints(agent, 6);
    let worklog = recent_worklog(agent, 20);
    let last_user = user_requests
        .last()
        .cloned()
        .unwrap_or_else(|| "No direct user request captured in recent history.".to_string());
    let last_assistant = assistant_messages
        .last()
        .cloned()
        .unwrap_or_else(|| "No assistant completion captured yet.".to_string());

    format!(
        "# Session Title\n{}\n\n# Current State\n- You were working in `{}`.\n- Latest user request: {}\n- Latest assistant state: {}\n- Session ID: {}\n\n# Task Specification\n{}\n\n# Files and Functions\n{}\n\n# Workflow\n{}\n\n# Errors & Corrections\n{}\n\n# Codebase and System Documentation\n- This artifact was generated by Holt from observed SDK session state.\n- It is a system-owned episodic snapshot, not a silent self-fork update.\n- Conversation continuity lives in the SDK thread; this file preserves resumable state for compaction and dream.\n- Working directory: `{}`\n\n# Learnings\n- Preserve the latest user goal and assistant progress in a resumable artifact.\n- Keep commands, file references, and error notes grounded in observed session data.\n- If richer extraction is added later, treat this snapshot as canonical input rather than disposable scaffolding.\n\n# Key Results\n{}\n\n# Worklog\n{}\n",
        sdk_session_title(agent, session_id),
        agent.working_directory.to_string_lossy(),
        last_user,
        last_assistant,
        session_id,
        bullet_list(&user_requests, "No user requests captured yet."),
        bullet_list(
            &file_hints,
            "No concrete file paths or function hints captured yet."
        ),
        bullet_list(&commands, "No shell or tool command history captured yet."),
        bullet_list(&error_hints, "No significant errors or corrections captured yet."),
        agent.working_directory.to_string_lossy(),
        bullet_list(
            &assistant_messages,
            "No assistant result captured yet."
        ),
        bullet_list(&worklog, "Session has not accumulated a worklog yet."),
    )
}

/// Context cap: min(80% of context window, 150K tokens).
fn context_cap(context_window_size: u32) -> u64 {
    let eighty_percent = (context_window_size as u64 * 80) / 100;
    std::cmp::min(eighty_percent, 150_000)
}

fn next_session_memory_id() -> String {
    format!("session-memory-{}", uuid::Uuid::new_v4())
}

pub fn session_memory_bridge_tags(session_id: &str, session_memory_id: &str) -> Vec<String> {
    vec![
        "session_memory".to_string(),
        "no_auto_inject".to_string(),
        format!("session:{}", session_id),
        format!("session_memory_id:{}", session_memory_id),
    ]
}

fn session_memory_bridge_content(
    session_id: &str,
    session_memory_id: &str,
    content: &str,
) -> String {
    format!(
        "[Session Memory Artifact]\nSession ID: {}\nArtifact ID: {}\n\n{}",
        session_id, session_memory_id, content
    )
}

async fn persist_session_memory_to_bridge(
    app_state: &Arc<crate::AppState>,
    agent_id: &str,
    session_id: &str,
    session_memory_id: &str,
    content: String,
) -> Result<SessionMemoryStoreReceipt, String> {
    let memory_engine = app_state
        .get_memory_engine()
        .ok_or_else(|| "Memory engine unavailable".to_string())?;

    let agent_ns = crate::memory::MemoryEngine::agent_namespace(agent_id);
    let tags = session_memory_bridge_tags(session_id, session_memory_id);
    let stored = memory_engine
        .store_detailed(StoreInput {
            content: session_memory_bridge_content(session_id, session_memory_id, &content),
            category: MemoryCategory::Episode,
            importance: 0.35,
            source: MemorySource::AutoCaptured,
            agent_ns: agent_ns.clone(),
            tier: MemoryTier::Warm,
            tags: tags.clone(),
        })
        .await
        .map_err(|e| e.to_string())?;

    Ok(SessionMemoryStoreReceipt {
        session_memory_id: session_memory_id.to_string(),
        agent_ns,
        session_id: session_id.to_string(),
        tags,
        stored_memory_id: stored.memory_id,
        bridge_status: stored.status,
        bridge_message: stored.message,
        verification_status: stored.verification_status,
    })
}

#[derive(Debug, Default)]
struct SessionMemoryFinalizeOutcome {
    warnings: Vec<String>,
    error: Option<String>,
    emit_updated: bool,
}

async fn finalize_session_memory_update<F, Fut>(
    session_path: &PathBuf,
    backup_path: &PathBuf,
    lifecycle_report: &mut SessionMemoryLifecycleReport,
    memory_engine_available: bool,
    persist_to_bridge: F,
) -> SessionMemoryFinalizeOutcome
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<SessionMemoryStoreReceipt, String>>,
{
    let Ok(content) = std::fs::read_to_string(session_path) else {
        lifecycle_report.validation_status = "read_failed".to_string();
        lifecycle_report.fork_error =
            Some("Failed to read session memory file after extraction".to_string());
        return SessionMemoryFinalizeOutcome {
            error: Some("Failed to read session memory file after extraction".to_string()),
            ..Default::default()
        };
    };

    if !validate_session_file(&content) {
        lifecycle_report.validation_status = "restored_from_backup".to_string();
        let _ = restore_from_backup(session_path, backup_path);
        return SessionMemoryFinalizeOutcome {
            error: Some("Session memory file corrupted — restored from backup".to_string()),
            ..Default::default()
        };
    }

    lifecycle_report.validation_status = "valid".to_string();
    let _ = std::fs::remove_file(backup_path);

    if lifecycle_report.fork_status != "completed" {
        lifecycle_report.bridge_store_status = Some("skipped_fork_incomplete".to_string());
    } else if content.trim().is_empty() {
        lifecycle_report.bridge_store_status = Some("skipped_empty".to_string());
    } else if memory_engine_available {
        lifecycle_report.bridge_store_attempted = true;
        match persist_to_bridge(content).await {
            Ok(receipt) => {
                lifecycle_report.bridge_store_status = Some(receipt.bridge_status.clone());
                lifecycle_report.receipt = Some(receipt);
            }
            Err(e) => {
                lifecycle_report.bridge_store_status = Some("failed".to_string());
                lifecycle_report.bridge_store_error = Some(e.clone());
                return SessionMemoryFinalizeOutcome {
                    warnings: vec![format!(
                        "Session memory updated locally, but Hillock store failed: {}",
                        e
                    )],
                    emit_updated: true,
                    ..Default::default()
                };
            }
        }
    } else {
        lifecycle_report.bridge_store_status = Some("skipped_unavailable".to_string());
    }

    SessionMemoryFinalizeOutcome {
        emit_updated: true,
        ..Default::default()
    }
}

/// Spawn a silent session memory extraction fork with deferred setup.
///
/// The caller (engine loop turn boundary) only does cheap trigger checks and
/// sets `extraction_in_progress = true`. ALL expensive work — context cloning,
/// file I/O, backup, fork construction — happens inside the async task.
///
/// Uses `background_rate_limiter` so extraction never competes with the user's
/// active conversation for rate limit tokens.
pub(crate) fn spawn_session_memory_fork(
    app_state: &Arc<crate::AppState>,
    agent_id: &str,
    session_id: &str,
    current_tokens: u64,
    app_handle: Option<&tauri::AppHandle>,
) {
    let app_state_clone = Arc::clone(app_state);
    let agent_id_owned = agent_id.to_string();
    let session_id_owned = session_id.to_string();
    let fork_app_handle = app_handle.cloned();

    tauri::async_runtime::spawn(async move {
        /// Drop guard that resets `extraction_in_progress` even if the future panics.
        struct ExtractionGuard {
            app_state: Arc<crate::AppState>,
            agent_id: String,
        }

        impl Drop for ExtractionGuard {
            fn drop(&mut self) {
                let s = Arc::clone(&self.app_state);
                let a = self.agent_id.clone();
                tauri::async_runtime::spawn(async move {
                    let mut agents_w = s.agents.write().await;
                    if let Some(agent) = agents_w.iter_mut().find(|ag| ag.id == a) {
                        agent.session_memory.extraction_in_progress = false;
                    }
                });
            }
        }

        let _guard = ExtractionGuard {
            app_state: Arc::clone(&app_state_clone),
            agent_id: agent_id_owned.clone(),
        };

        // Helper to reset the extraction_in_progress flag on any exit path
        let reset_flag = |state: &Arc<crate::AppState>, aid: &str| {
            let s = Arc::clone(state);
            let a = aid.to_string();
            tauri::async_runtime::spawn(async move {
                let mut agents_w = s.agents.write().await;
                if let Some(agent) = agents_w.iter_mut().find(|ag| ag.id == a) {
                    agent.session_memory.extraction_in_progress = false;
                }
            });
        };

        // DEFERRED: Acquire read lock and build fork context
        let fork_data = {
            let agents_read = app_state_clone.agents.read().await;
            agents_read
                .iter()
                .find(|a| a.id == agent_id_owned)
                .map(|agent| {
                    (
                        build_fork_context(agent),
                        agent.protocol.clone(),
                        agent.anthropic_settings.clone(),
                        agent.working_directory.clone(),
                        agent.context_window_size,
                    )
                })
        };

        let Some((
            fork_context,
            protocol,
            anthropic_settings,
            working_directory,
            context_window_size,
        )) = fork_data
        else {
            // Expected race: the parent/helper agent can be removed before this
            // deferred extraction task acquires the read lock.
            tracing::debug!(agent_id = %agent_id_owned, "Session memory: agent disappeared before deferred extraction started");
            reset_flag(&app_state_clone, &agent_id_owned);
            return;
        };

        // DEFERRED: File I/O — template, content read, backup
        let session_memory_id = next_session_memory_id();
        let session_path = session_memory_path(&agent_id_owned, &session_id_owned);
        ensure_template_exists(&session_path);

        let current_content = std::fs::read_to_string(&session_path).unwrap_or_default();

        let mut lifecycle_report = SessionMemoryLifecycleReport {
            source: "session_memory_fork".to_string(),
            agent_id: agent_id_owned.clone(),
            agent_ns: Some(crate::memory::MemoryEngine::agent_namespace(
                &agent_id_owned,
            )),
            session_id: session_id_owned.clone(),
            session_memory_id: session_memory_id.clone(),
            trigger_tokens: current_tokens,
            context_cap: 0,
            session_file_path: session_path.to_string_lossy().to_string(),
            backup_file_path: None,
            fork_status: "spawned".to_string(),
            fork_error: None,
            fork_turns: None,
            validation_status: "pending".to_string(),
            bridge_store_attempted: false,
            bridge_store_status: None,
            bridge_store_error: None,
            receipt: None,
        };

        let backup_path = match backup_session_file(&session_path) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(agent_id = %agent_id_owned, "Session memory backup failed: {e}");
                lifecycle_report.fork_status = "backup_failed".to_string();
                lifecycle_report.fork_error = Some(e.to_string());
                lifecycle_report.validation_status = "not_run".to_string();
                let _ = write_session_memory_lifecycle_trace(
                    &app_state_clone.trace_engine,
                    &lifecycle_report,
                );
                reset_flag(&app_state_clone, &agent_id_owned);
                return;
            }
        };
        lifecycle_report.backup_file_path = Some(backup_path.to_string_lossy().to_string());

        // Emit warning if context exceeds cap
        let cap = context_cap(context_window_size);
        lifecycle_report.context_cap = cap;
        if current_tokens > cap {
            if let Some(handle) = &fork_app_handle {
                let _ = handle.emit("agent-stream", crate::providers::types::StreamEvent {
                    agent_id: agent_id_owned.clone(),
                    event_type: crate::providers::types::StreamEventType::SessionMemoryWarning,
                    data: serde_json::json!({
                        "message": "Session context is large — consider compacting for best results",
                        "current_tokens": current_tokens,
                        "cap": cap,
                    }),
                });
            }
        }

        let path_str = session_path.to_string_lossy().to_string();
        let extraction_prompt = build_extraction_prompt(&path_str, &current_content);
        let codex_extraction_prompt = build_codex_extraction_prompt(&path_str, &current_content);

        let parent_assistant_count = fork_context
            .context_messages
            .iter()
            .filter(|m| m.role == crate::runtime::agent::MessageRole::Assistant)
            .count();

        // Build fork AgentSlot
        let fork_id = uuid::Uuid::new_v4().to_string();
        let is_codex_protocol = protocol == AgentProtocol::Codex;
        let fork_slot = crate::runtime::agent::AgentSlot {
            id: fork_id.clone(),
            name: format!("session-memory-{}", &fork_id[..8]),
            color: crate::runtime::agent::AgentColor::default(),
            protected: false,
            connection: fork_context.connection.clone(),
            protocol,
            working_directory: working_directory.clone(),
            status: crate::runtime::agent::AgentStatus::Idle,
            subagents: vec![],
            conversation: crate::runtime::agent::ConversationHistory {
                messages: fork_context.context_messages.clone(),
                total_token_estimate: 0,
            },
            tools: vec![],
            system_prompt: fork_context.system_prompt.clone(),
            metadata: crate::runtime::agent::AgentMetadata::default(),
            limits: crate::runtime::limits::AgentLimits::default(),
            context_window_size: 125_000,
            context_tokens_override: None,
            context_probed: false,
            sandbox_config: crate::config::agent_config::BuildSandboxBlock::default(),
            sandbox: None,
            session_prompt_tokens: 0,
            session_completion_tokens: 0,
            current_context_tokens: 0,
            session_start: None,
            is_dirty: false,
            anthropic_settings,
            skills: vec![],
            user_notes: vec![],
            agent_session_id: None,
            system_prompt_hash: None,
            coordinator_mode: false,
            autonomous: false,
            extraction_pending_done: false,
            provider: None,
            cached_params: None,
            multi_agent_model: false,
            archetype_base: None,
            archetype_specialization: None,
            session_memory: SessionMemoryState::default(),
            cognitive_intake: Default::default(),
            appearance: None,
            openai_responses_settings: None,
            xai_settings: None,
            gemini_settings: None,
            local_model_settings: None,
        };

        let fork_agents: Arc<tokio::sync::RwLock<Vec<crate::runtime::agent::AgentSlot>>> =
            Arc::new(tokio::sync::RwLock::new(vec![fork_slot]));

        let fork_id_clone = fork_id.clone();
        let session_path_clone = session_path.clone();
        let backup_path_clone = backup_path.clone();

        let config = app_state_clone.config.read().await;
        let network_config = config.network.clone();
        let limits_config = config.limits.clone();
        drop(config);

        let cancel_token = tokio_util::sync::CancellationToken::new();

        let fork_directive = format!(
            "[You are a session memory extractor — a silent background process. \
            Do NOT continue or respond to any prior conversation tasks. \
            Your ONLY job is to update the session notes file.]\n\n{}",
            extraction_prompt
        );
        let codex_fork_directive = format!(
            "[You are a session memory extractor — a silent background process. \
            Do NOT continue or respond to any prior conversation tasks. \
            Your ONLY job is to produce the updated session notes document.]\n\n{}",
            codex_extraction_prompt
        );

        // 5-minute timeout, 10-turn cap for engine-backed lanes. Codex runs a
        // single bounded exec and Holt writes the returned markdown.
        let result = if is_codex_protocol {
            tokio::time::timeout(std::time::Duration::from_secs(300), async {
                let updated_markdown = crate::runtime::codex::run_codex_agent(
                    &fork_agents,
                    &fork_id_clone,
                    &codex_fork_directive,
                    None,
                    &[],
                    None,
                    None,
                    &cancel_token,
                    Some(&app_state_clone),
                    false,
                )
                .await
                .map_err(|e| e.to_string())?;

                std::fs::write(&session_path_clone, updated_markdown)
                    .map_err(|e| format!("Failed to write session memory file: {e}"))?;

                Result::<(), String>::Ok(())
            })
            .await
        } else {
            let tool_registry = crate::tools::registry::ToolRegistry::build_for_session_memory(
                session_path.clone(),
            );

            let engine_ctx = crate::runtime::engine::EngineContext {
                agents: &fork_agents,
                agent_id: &fork_id_clone,
                trace_engine: &app_state_clone.trace_engine,
                keychain: &app_state_clone.keychain,
                rate_limiter: &app_state_clone.background_rate_limiter,
                network_config: &network_config,
                limits_config: &limits_config,
                app_handle: None,
                tool_registry: &tool_registry,
                shell_manager: &app_state_clone.shell_manager,
                app_config: &app_state_clone.config,
                approval_manager: &app_state_clone.approval_manager,
                vfl_registry: &app_state_clone.vfl_registry,
                subagent_manager: &app_state_clone.subagent_manager,
                a2a_registry: &app_state_clone.a2a_registry,
                cancellation_token: Some(&cancel_token),
                app_state: Some(Arc::clone(&app_state_clone)),
                suppress_memory_pipeline: true,
                transient_user_message: None,
            };

            tokio::time::timeout(std::time::Duration::from_secs(300), async {
                crate::runtime::engine::run_agentic_loop(
                    &engine_ctx,
                    &fork_directive,
                    None,
                    Some(10),
                )
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
            })
            .await
        };

        // The codex lane spawned an app-server keyed by the throwaway fork id;
        // without an explicit shutdown it outlives the extraction forever
        // (leaked process + auth.json refresh races).
        if is_codex_protocol {
            crate::runtime::codex_server::shutdown_server(&fork_id_clone)
                .await
                .ok();
        }

        // Log result and emit errors to frontend
        match &result {
            Ok(Ok(_)) => {
                lifecycle_report.fork_status = "completed".to_string();
                tracing::info!(agent_id = %agent_id_owned, "Session memory extraction completed");
            }
            Ok(Err(e)) => {
                lifecycle_report.fork_status = "engine_error".to_string();
                lifecycle_report.fork_error = Some(e.to_string());
                tracing::warn!(agent_id = %agent_id_owned, "Session memory extraction engine error: {e}");
                if let Some(handle) = &fork_app_handle {
                    let _ = handle.emit(
                        "agent-stream",
                        crate::providers::types::StreamEvent {
                            agent_id: agent_id_owned.clone(),
                            event_type:
                                crate::providers::types::StreamEventType::SessionMemoryError,
                            data: serde_json::json!({
                                "message": format!("Session memory extraction failed: {e}"),
                            }),
                        },
                    );
                }
            }
            Err(_) => {
                cancel_token.cancel();
                lifecycle_report.fork_status = "timeout".to_string();
                lifecycle_report.fork_error =
                    Some("Session memory extraction timed out".to_string());
                tracing::warn!(agent_id = %agent_id_owned, "Session memory extraction timed out (5m)");
                if let Some(handle) = &fork_app_handle {
                    let _ = handle.emit(
                        "agent-stream",
                        crate::providers::types::StreamEvent {
                            agent_id: agent_id_owned.clone(),
                            event_type:
                                crate::providers::types::StreamEventType::SessionMemoryError,
                            data: serde_json::json!({
                                "message": "Session memory extraction timed out",
                            }),
                        },
                    );
                }
            }
        }

        // Check if extraction took too many turns
        {
            let fork_agents_r = fork_agents.read().await;
            if let Some(fork_agent) = fork_agents_r.iter().find(|a| a.id == fork_id_clone) {
                let assistant_turns = fork_agent
                    .conversation
                    .messages
                    .iter()
                    .filter(|m| m.role == crate::runtime::agent::MessageRole::Assistant)
                    .count();
                let fork_turns = assistant_turns.saturating_sub(parent_assistant_count);
                lifecycle_report.fork_turns = Some(fork_turns);
                if fork_turns > 3 {
                    tracing::warn!(
                        agent_id = %agent_id_owned,
                        turns = fork_turns,
                        "Session memory extraction took {} turns (expected 1-2)",
                        fork_turns
                    );
                    if let Some(handle) = &fork_app_handle {
                        let _ = handle.emit("agent-stream", crate::providers::types::StreamEvent {
                            agent_id: agent_id_owned.clone(),
                            event_type: crate::providers::types::StreamEventType::SessionMemoryWarning,
                            data: serde_json::json!({
                                "message": format!("Session memory extraction took {} turns (expected 1-2)", fork_turns),
                            }),
                        });
                    }
                }
            }
        }

        // Post-extraction validation + parent-side bridge receipt
        let finalize_outcome = finalize_session_memory_update(
            &session_path_clone,
            &backup_path_clone,
            &mut lifecycle_report,
            app_state_clone.has_memory_engine(),
            |content| {
                persist_session_memory_to_bridge(
                    &app_state_clone,
                    &agent_id_owned,
                    &session_id_owned,
                    &session_memory_id,
                    content,
                )
            },
        )
        .await;

        for warning in &finalize_outcome.warnings {
            tracing::warn!(agent_id = %agent_id_owned, "{warning}");
            if let Some(handle) = &fork_app_handle {
                let _ = handle.emit(
                    "agent-stream",
                    crate::providers::types::StreamEvent {
                        agent_id: agent_id_owned.clone(),
                        event_type: crate::providers::types::StreamEventType::SessionMemoryWarning,
                        data: serde_json::json!({
                            "message": warning,
                        }),
                    },
                );
            }
        }

        if let Some(error_message) = &finalize_outcome.error {
            tracing::warn!(agent_id = %agent_id_owned, "{error_message}");
            if let Some(handle) = &fork_app_handle {
                let _ = handle.emit(
                    "agent-stream",
                    crate::providers::types::StreamEvent {
                        agent_id: agent_id_owned.clone(),
                        event_type: crate::providers::types::StreamEventType::SessionMemoryError,
                        data: serde_json::json!({
                            "message": error_message,
                        }),
                    },
                );
            }
        }

        if finalize_outcome.emit_updated {
            if let Some(handle) = &fork_app_handle {
                let _ = handle.emit(
                    "agent-stream",
                    crate::providers::types::StreamEvent {
                        agent_id: agent_id_owned.clone(),
                        event_type: crate::providers::types::StreamEventType::SessionMemoryUpdated,
                        data: serde_json::json!({
                            "message": "Session memory updated",
                            "session_file_path": session_path_clone.to_string_lossy(),
                            "session_memory_id": session_memory_id,
                            "bridge_store_status": lifecycle_report.bridge_store_status.clone(),
                            "bridge_store_error": lifecycle_report.bridge_store_error.clone(),
                            "receipt": lifecycle_report.receipt.clone(),
                        }),
                    },
                );
            }
        }

        let _ =
            write_session_memory_lifecycle_trace(&app_state_clone.trace_engine, &lifecycle_report);

        // Reset extraction state on parent agent
        let current_tokens = {
            let agents_r = app_state_clone.agents.read().await;
            agents_r
                .iter()
                .find(|a| a.id == agent_id_owned)
                .map(|a| a.current_context_tokens)
                .unwrap_or(0)
        };
        {
            let mut agents_w = app_state_clone.agents.write().await;
            if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_owned) {
                agent
                    .session_memory
                    .mark_extraction_complete(current_tokens);
            }
        }
    });

    tracing::debug!(
        agent_id,
        "Session memory extraction fork spawned (deferred setup)"
    );
}

/// Spawn a system-owned session memory capture for SDK lanes.
///
/// Unlike `spawn_session_memory_fork`, this does not ask the lane to silently
/// fork itself. It derives the canonical session-memory artifact from observed
/// session state (conversation, tool receipts, working directory, session ID)
/// and stores it in the same file/Hillock locations as the non-SDK path.
pub(crate) fn spawn_sdk_session_memory_capture(
    app_state: &Arc<crate::AppState>,
    agent_id: &str,
    session_id: &str,
    current_tokens: u64,
    app_handle: Option<&tauri::AppHandle>,
) {
    let app_state_clone = Arc::clone(app_state);
    let agent_id_owned = agent_id.to_string();
    let session_id_owned = session_id.to_string();
    let app_handle_clone = app_handle.cloned();

    tauri::async_runtime::spawn(async move {
        let session_memory_id = next_session_memory_id();
        let session_path = session_memory_path(&agent_id_owned, &session_id_owned);
        ensure_template_exists(&session_path);

        let mut lifecycle_report = SessionMemoryLifecycleReport {
            source: "sdk_session_memory_snapshot".to_string(),
            agent_id: agent_id_owned.clone(),
            agent_ns: Some(crate::memory::MemoryEngine::agent_namespace(
                &agent_id_owned,
            )),
            session_id: session_id_owned.clone(),
            session_memory_id: session_memory_id.clone(),
            trigger_tokens: current_tokens,
            context_cap: context_cap(125_000),
            session_file_path: session_path.to_string_lossy().to_string(),
            backup_file_path: None,
            fork_status: "spawned".to_string(),
            fork_error: None,
            fork_turns: Some(0),
            validation_status: "pending".to_string(),
            bridge_store_attempted: false,
            bridge_store_status: None,
            bridge_store_error: None,
            receipt: None,
        };

        let backup_path = match backup_session_file(&session_path) {
            Ok(path) => path,
            Err(e) => {
                lifecycle_report.fork_status = "backup_failed".to_string();
                lifecycle_report.fork_error = Some(e.to_string());
                lifecycle_report.validation_status = "not_run".to_string();
                let _ = write_session_memory_lifecycle_trace(
                    &app_state_clone.trace_engine,
                    &lifecycle_report,
                );
                let mut agents_w = app_state_clone.agents.write().await;
                if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_owned) {
                    agent.session_memory.extraction_in_progress = false;
                }
                return;
            }
        };
        lifecycle_report.backup_file_path = Some(backup_path.to_string_lossy().to_string());

        let snapshot = {
            let agents_r = app_state_clone.agents.read().await;
            agents_r
                .iter()
                .find(|a| a.id == agent_id_owned)
                .cloned()
                .map(|agent| build_sdk_session_memory_snapshot(&agent, &session_id_owned))
        };

        let Some(snapshot) = snapshot else {
            lifecycle_report.fork_status = "agent_missing".to_string();
            lifecycle_report.fork_error =
                Some("Agent disappeared before SDK session memory capture".to_string());
            lifecycle_report.validation_status = "not_run".to_string();
            let _ = write_session_memory_lifecycle_trace(
                &app_state_clone.trace_engine,
                &lifecycle_report,
            );
            let mut agents_w = app_state_clone.agents.write().await;
            if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_owned) {
                agent.session_memory.extraction_in_progress = false;
            }
            return;
        };

        if let Err(e) = std::fs::write(&session_path, snapshot) {
            lifecycle_report.fork_status = "write_failed".to_string();
            lifecycle_report.fork_error = Some(e.to_string());
            lifecycle_report.validation_status = "not_run".to_string();
            let _ = restore_from_backup(&session_path, &backup_path);
            let _ = write_session_memory_lifecycle_trace(
                &app_state_clone.trace_engine,
                &lifecycle_report,
            );
            if let Some(handle) = &app_handle_clone {
                let _ = handle.emit(
                    "agent-stream",
                    crate::providers::types::StreamEvent {
                        agent_id: agent_id_owned.clone(),
                        event_type: crate::providers::types::StreamEventType::SessionMemoryError,
                        data: serde_json::json!({
                            "message": format!("Failed to write SDK session memory: {}", e),
                        }),
                    },
                );
            }
            let mut agents_w = app_state_clone.agents.write().await;
            if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_owned) {
                agent.session_memory.extraction_in_progress = false;
            }
            return;
        }

        lifecycle_report.fork_status = "completed".to_string();

        let finalize_outcome = finalize_session_memory_update(
            &session_path,
            &backup_path,
            &mut lifecycle_report,
            app_state_clone.has_memory_engine(),
            {
                let app_state_for_persist = Arc::clone(&app_state_clone);
                let agent_id_for_persist = agent_id_owned.clone();
                let session_id_for_persist = session_id_owned.clone();
                let session_memory_id_for_persist = session_memory_id.clone();
                move |content| async move {
                    persist_session_memory_to_bridge(
                        &app_state_for_persist,
                        &agent_id_for_persist,
                        &session_id_for_persist,
                        &session_memory_id_for_persist,
                        content,
                    )
                    .await
                }
            },
        )
        .await;

        for warning in &finalize_outcome.warnings {
            if let Some(handle) = &app_handle_clone {
                let _ = handle.emit(
                    "agent-stream",
                    crate::providers::types::StreamEvent {
                        agent_id: agent_id_owned.clone(),
                        event_type: crate::providers::types::StreamEventType::SessionMemoryWarning,
                        data: serde_json::json!({ "message": warning }),
                    },
                );
            }
        }

        if let Some(error_message) = &finalize_outcome.error {
            if let Some(handle) = &app_handle_clone {
                let _ = handle.emit(
                    "agent-stream",
                    crate::providers::types::StreamEvent {
                        agent_id: agent_id_owned.clone(),
                        event_type: crate::providers::types::StreamEventType::SessionMemoryError,
                        data: serde_json::json!({ "message": error_message }),
                    },
                );
            }
        }

        if finalize_outcome.emit_updated {
            if let Some(handle) = &app_handle_clone {
                let _ = handle.emit(
                    "agent-stream",
                    crate::providers::types::StreamEvent {
                        agent_id: agent_id_owned.clone(),
                        event_type: crate::providers::types::StreamEventType::SessionMemoryUpdated,
                        data: serde_json::json!({
                            "message": "Session memory updated",
                            "session_file_path": session_path.to_string_lossy(),
                            "session_memory_id": session_memory_id,
                            "bridge_store_status": lifecycle_report.bridge_store_status.clone(),
                            "bridge_store_error": lifecycle_report.bridge_store_error.clone(),
                            "receipt": lifecycle_report.receipt.clone(),
                        }),
                    },
                );
            }
        }

        let _ =
            write_session_memory_lifecycle_trace(&app_state_clone.trace_engine, &lifecycle_report);

        let mut agents_w = app_state_clone.agents.write().await;
        if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id_owned) {
            if finalize_outcome.emit_updated {
                agent
                    .session_memory
                    .mark_extraction_complete(current_tokens);
            } else {
                agent.session_memory.extraction_in_progress = false;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::agent::{
        AgentColor, AgentMetadata, AgentSlot, ConversationHistory, ConversationMessage,
    };
    use crate::runtime::appearance::AgentAppearance;
    use crate::runtime::connection::{AgentProtocol, ConnectionConfig};
    use chrono::Utc;
    use std::path::PathBuf;

    #[test]
    fn default_state_is_not_ready_for_extraction() {
        let state = SessionMemoryState::default();
        let config = SessionMemoryConfig::default();
        assert!(!state.should_trigger(&config, 0));
    }

    #[test]
    fn no_trigger_below_init_threshold() {
        let state = SessionMemoryState::default();
        let config = SessionMemoryConfig::default();
        assert!(!state.should_trigger(&config, 9_999));
    }

    #[test]
    fn first_extraction_triggers_at_init_threshold_with_activity() {
        let mut state = SessionMemoryState::default();
        state.tool_calls_since_extraction = 3;
        let config = SessionMemoryConfig::default();
        assert!(state.should_trigger(&config, 10_000));
    }

    #[test]
    fn no_trigger_without_activity() {
        let state = SessionMemoryState::default();
        let config = SessionMemoryConfig::default();
        assert!(!state.should_trigger(&config, 15_000));
    }

    #[test]
    fn trigger_with_messages_instead_of_tool_calls() {
        let mut state = SessionMemoryState::default();
        state.assistant_messages_since_extraction = 8;
        let config = SessionMemoryConfig::default();
        assert!(state.should_trigger(&config, 10_000));
    }

    #[test]
    fn no_trigger_when_extraction_in_progress() {
        let mut state = SessionMemoryState::default();
        state.tool_calls_since_extraction = 5;
        state.extraction_in_progress = true;
        let config = SessionMemoryConfig::default();
        assert!(!state.should_trigger(&config, 15_000));
    }

    #[test]
    fn subsequent_extraction_needs_growth() {
        let mut state = SessionMemoryState::default();
        state.initialized = true;
        state.tokens_at_last_extraction = 10_000;
        state.tool_calls_since_extraction = 5;
        let config = SessionMemoryConfig::default();
        assert!(!state.should_trigger(&config, 14_999));
        assert!(state.should_trigger(&config, 15_000));
    }

    #[test]
    fn session_memory_path_uses_agent_and_session_id() {
        let path = session_memory_path("agent-1", "sess-abc");
        assert!(path.ends_with("agents/agent-1/memory/sessions/sess-abc.md"));
    }

    #[test]
    fn session_memory_bridge_tags_include_session_and_exclusion_markers() {
        let tags = session_memory_bridge_tags("sess-123", "session-memory-xyz");
        assert!(tags.contains(&"session_memory".to_string()));
        assert!(tags.contains(&"no_auto_inject".to_string()));
        assert!(tags.contains(&"session:sess-123".to_string()));
        assert!(tags.contains(&"session_memory_id:session-memory-xyz".to_string()));
    }

    #[test]
    fn session_memory_template_has_all_ten_sections() {
        let template = session_memory_template();
        for header in REQUIRED_HEADERS {
            assert!(template.contains(header), "Missing header: {}", header);
        }
    }

    #[test]
    fn validate_session_file_accepts_valid_template() {
        let template = session_memory_template();
        assert!(validate_session_file(&template));
    }

    #[test]
    fn validate_session_file_rejects_missing_header() {
        let broken = "# Session Title\n# Current State\n";
        assert!(!validate_session_file(broken));
    }

    #[test]
    fn template_created_on_disk_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.md");
        assert!(!path.exists());
        ensure_template_exists(&path);
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(validate_session_file(&content));
    }

    #[test]
    fn template_not_overwritten_when_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.md");
        std::fs::write(&path, "# Session Title\ncustom content").unwrap();
        ensure_template_exists(&path);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("custom content"));
    }

    #[test]
    fn backup_and_restore_on_validation_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.md");
        let valid = session_memory_template();
        std::fs::write(&path, &valid).unwrap();

        let backup = backup_session_file(&path).unwrap();
        assert!(backup.exists());

        // Corrupt the file
        std::fs::write(&path, "# Session Title\nonly one header").unwrap();
        assert!(!validate_session_file(
            &std::fs::read_to_string(&path).unwrap()
        ));

        // Restore from backup
        restore_from_backup(&path, &backup).unwrap();
        let restored = std::fs::read_to_string(&path).unwrap();
        assert!(validate_session_file(&restored));
    }

    #[test]
    fn extraction_prompt_contains_path_and_content() {
        let prompt = build_extraction_prompt("/tmp/session.md", "# Session Title\ntest");
        assert!(prompt.contains("/tmp/session.md"));
        assert!(prompt.contains("test"));
    }

    #[test]
    fn codex_extraction_prompt_requests_full_markdown_response() {
        let prompt = build_codex_extraction_prompt("/tmp/session.md", "# Session Title\ntest");
        assert!(prompt.contains("/tmp/session.md"));
        assert!(prompt.contains("test"));
        assert!(prompt.contains("Return the COMPLETE updated markdown document only"));
        assert!(prompt.contains("Do NOT call tools"));
        assert!(prompt.contains("Do NOT wrap the response in code fences"));
    }

    fn test_agent_slot() -> AgentSlot {
        AgentSlot {
            id: "agent-1".to_string(),
            name: "Codex".to_string(),
            color: AgentColor::default(),
            protected: false,
            connection: ConnectionConfig::AgentSdk,
            protocol: AgentProtocol::AgentSdk,
            working_directory: PathBuf::from("/tmp/workspace"),
            status: crate::runtime::agent::AgentStatus::Idle,
            subagents: vec![],
            conversation: ConversationHistory::default(),
            tools: vec![],
            system_prompt: String::new(),
            metadata: AgentMetadata::default(),
            limits: Default::default(),
            context_window_size: 125_000,
            context_tokens_override: None,
            context_probed: false,
            sandbox_config: Default::default(),
            sandbox: None,
            session_prompt_tokens: 0,
            session_completion_tokens: 0,
            current_context_tokens: 12_000,
            session_start: None,
            is_dirty: false,
            anthropic_settings: None,
            skills: vec![],
            user_notes: vec![],
            agent_session_id: Some("sess-123".to_string()),
            system_prompt_hash: None,
            coordinator_mode: false,
            autonomous: true,
            extraction_pending_done: false,
            provider: None,
            cached_params: None,
            multi_agent_model: false,
            archetype_base: None,
            archetype_specialization: None,
            session_memory: SessionMemoryState::default(),
            cognitive_intake: Default::default(),
            appearance: Some(AgentAppearance::default()),
            openai_responses_settings: None,
            xai_settings: None,
            gemini_settings: None,
            local_model_settings: None,
        }
    }

    #[test]
    fn sdk_snapshot_contains_required_sections_and_commands() {
        let mut agent = test_agent_slot();
        agent.conversation.messages.push(ConversationMessage {
            role: MessageRole::User,
            content: "Refactor app/src/lib/foo.ts and then run npm run check".to_string(),
            content_blocks: None,
            timestamp: Utc::now(),
            token_estimate: None,
            tool_calls: None,
            tool_call_id: None,
            outcome_tag: None,
            thinking_content: None,
            responses_phase: None,
            reasoning_items: None,
        });
        agent.conversation.messages.push(ConversationMessage {
            role: MessageRole::Tool,
            content: serde_json::json!({
                "command": "npm run check",
                "output": "0 errors, 0 warnings",
                "exit_code": 0
            })
            .to_string(),
            content_blocks: None,
            timestamp: Utc::now(),
            token_estimate: None,
            tool_calls: None,
            tool_call_id: Some("command_execution".to_string()),
            outcome_tag: None,
            thinking_content: None,
            responses_phase: None,
            reasoning_items: None,
        });
        agent.conversation.messages.push(ConversationMessage {
            role: MessageRole::Assistant,
            content: "Check passed and foo.ts is in a cleaner state now.".to_string(),
            content_blocks: None,
            timestamp: Utc::now(),
            token_estimate: None,
            tool_calls: None,
            tool_call_id: None,
            outcome_tag: None,
            thinking_content: None,
            responses_phase: None,
            reasoning_items: None,
        });

        let snapshot = build_sdk_session_memory_snapshot(&agent, "sess-123");
        assert!(validate_session_file(&snapshot));
        assert!(snapshot.contains("npm run check"));
        assert!(snapshot.contains("foo.ts"));
        assert!(snapshot.contains("Session ID: sess-123"));
    }

    fn test_lifecycle_report(
        session_path: &std::path::Path,
        backup_path: &std::path::Path,
    ) -> SessionMemoryLifecycleReport {
        SessionMemoryLifecycleReport {
            source: "session_memory_fork".to_string(),
            agent_id: "agent-1".to_string(),
            agent_ns: Some("agent_1".to_string()),
            session_id: "sess-123".to_string(),
            session_memory_id: "session-memory-xyz".to_string(),
            trigger_tokens: 12_000,
            context_cap: 100_000,
            session_file_path: session_path.to_string_lossy().to_string(),
            backup_file_path: Some(backup_path.to_string_lossy().to_string()),
            fork_status: "completed".to_string(),
            fork_error: None,
            fork_turns: Some(1),
            validation_status: "pending".to_string(),
            bridge_store_attempted: false,
            bridge_store_status: None,
            bridge_store_error: None,
            receipt: None,
        }
    }

    fn test_receipt() -> SessionMemoryStoreReceipt {
        SessionMemoryStoreReceipt {
            session_memory_id: "session-memory-xyz".to_string(),
            agent_ns: "agent_1".to_string(),
            session_id: "sess-123".to_string(),
            tags: vec![
                "session_memory".to_string(),
                "no_auto_inject".to_string(),
                "session:sess-123".to_string(),
            ],
            stored_memory_id: "memory:abc123".to_string(),
            bridge_status: "created".to_string(),
            bridge_message: "stored".to_string(),
            verification_status: "not_required".to_string(),
        }
    }

    #[tokio::test]
    async fn finalize_session_memory_persists_valid_content_and_records_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let session_path = dir.path().join("session.md");
        let backup_path = dir.path().join("session.md.bak");
        let content = session_memory_template();
        std::fs::write(&session_path, &content).unwrap();
        std::fs::write(&backup_path, &content).unwrap();

        let mut report = test_lifecycle_report(&session_path, &backup_path);
        let outcome = finalize_session_memory_update(
            &session_path,
            &backup_path,
            &mut report,
            true,
            |_| async { Ok(test_receipt()) },
        )
        .await;

        assert!(outcome.error.is_none());
        assert!(outcome.warnings.is_empty());
        assert!(outcome.emit_updated);
        assert_eq!(report.validation_status, "valid");
        assert!(report.bridge_store_attempted);
        assert_eq!(report.bridge_store_status.as_deref(), Some("created"));
        assert_eq!(
            report
                .receipt
                .as_ref()
                .map(|receipt| receipt.stored_memory_id.as_str()),
            Some("memory:abc123")
        );
        assert!(!backup_path.exists());
    }

    #[tokio::test]
    async fn finalize_session_memory_restores_backup_on_invalid_content() {
        let dir = tempfile::tempdir().unwrap();
        let session_path = dir.path().join("session.md");
        let backup_path = dir.path().join("session.md.bak");
        let valid = session_memory_template();
        std::fs::write(&session_path, "# Session Title\nbroken").unwrap();
        std::fs::write(&backup_path, &valid).unwrap();

        let mut report = test_lifecycle_report(&session_path, &backup_path);
        let outcome = finalize_session_memory_update(
            &session_path,
            &backup_path,
            &mut report,
            true,
            |_| async { panic!("persist_to_bridge should not be called for invalid content") },
        )
        .await;

        assert_eq!(
            outcome.error.as_deref(),
            Some("Session memory file corrupted — restored from backup")
        );
        assert!(!outcome.emit_updated);
        assert_eq!(report.validation_status, "restored_from_backup");
        assert!(!report.bridge_store_attempted);
        assert!(validate_session_file(
            &std::fs::read_to_string(&session_path).unwrap()
        ));
        assert!(!backup_path.exists());
    }

    #[tokio::test]
    async fn finalize_session_memory_skips_bridge_when_fork_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let session_path = dir.path().join("session.md");
        let backup_path = dir.path().join("session.md.bak");
        let content = session_memory_template();
        std::fs::write(&session_path, &content).unwrap();
        std::fs::write(&backup_path, &content).unwrap();

        let mut report = test_lifecycle_report(&session_path, &backup_path);
        report.fork_status = "engine_error".to_string();

        let outcome = finalize_session_memory_update(
            &session_path,
            &backup_path,
            &mut report,
            true,
            |_| async { panic!("persist_to_bridge should not be called when fork is incomplete") },
        )
        .await;

        assert!(outcome.error.is_none());
        assert!(outcome.emit_updated);
        assert_eq!(report.validation_status, "valid");
        assert!(!report.bridge_store_attempted);
        assert_eq!(
            report.bridge_store_status.as_deref(),
            Some("skipped_fork_incomplete")
        );
        assert!(report.receipt.is_none());
        assert!(!backup_path.exists());
    }

    #[tokio::test]
    async fn finalize_session_memory_skips_bridge_when_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let session_path = dir.path().join("session.md");
        let backup_path = dir.path().join("session.md.bak");
        let content = session_memory_template();
        std::fs::write(&session_path, &content).unwrap();
        std::fs::write(&backup_path, &content).unwrap();

        let mut report = test_lifecycle_report(&session_path, &backup_path);
        let outcome = finalize_session_memory_update(
            &session_path,
            &backup_path,
            &mut report,
            false,
            |_| async {
                panic!("persist_to_bridge should not be called when memory is unavailable")
            },
        )
        .await;

        assert!(outcome.error.is_none());
        assert!(outcome.emit_updated);
        assert_eq!(report.validation_status, "valid");
        assert!(!report.bridge_store_attempted);
        assert_eq!(
            report.bridge_store_status.as_deref(),
            Some("skipped_unavailable")
        );
        assert!(report.receipt.is_none());
    }

    #[tokio::test]
    async fn finalize_session_memory_records_warning_when_bridge_store_fails() {
        let dir = tempfile::tempdir().unwrap();
        let session_path = dir.path().join("session.md");
        let backup_path = dir.path().join("session.md.bak");
        let content = session_memory_template();
        std::fs::write(&session_path, &content).unwrap();
        std::fs::write(&backup_path, &content).unwrap();

        let mut report = test_lifecycle_report(&session_path, &backup_path);
        let outcome = finalize_session_memory_update(
            &session_path,
            &backup_path,
            &mut report,
            true,
            |_| async { Err("bridge offline".to_string()) },
        )
        .await;

        assert!(outcome.error.is_none());
        assert!(outcome.emit_updated);
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("Hillock store failed"));
        assert_eq!(report.validation_status, "valid");
        assert!(report.bridge_store_attempted);
        assert_eq!(report.bridge_store_status.as_deref(), Some("failed"));
        assert_eq!(report.bridge_store_error.as_deref(), Some("bridge offline"));
        assert!(report.receipt.is_none());
        assert!(!backup_path.exists());
    }
}
