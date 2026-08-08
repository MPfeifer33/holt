//! Compaction lifecycle — memory extraction before context eviction.
//!
//! Flow:
//! 1. Pre-emptive extraction at 150K tokens (configurable)
//! 2. LLM extraction: send conversation to agent's model for key fact extraction
//! 3. Write active_work.md to agent's persona folder
//! 4. Store extracted memories to bridge (warm tier)
//! 5. Store compaction summary as state memory (high importance)
//!
//! Post-compaction detection happens in session.rs via update_context_tokens().

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::memory::store::{MemoryCategory, MemorySource, MemoryTier, StoreInput};
use crate::memory::MemoryEngine;
use crate::runtime::agent::AgentSlot;

/// Result of a pre-compaction extraction attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum ExtractionOutcome {
    /// Extraction ran successfully.
    Completed,
    /// Extraction was not needed (below threshold, already done, empty conversation).
    NotNeeded,
    /// Extraction ran but storage failed — safe to retry next cycle.
    StoreFailed(String),
}

/// Structured output from LLM pre-compaction extraction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractionResult {
    /// ISO 8601 timestamp — set by Holt after parsing, not by the LLM.
    #[serde(default)]
    pub extracted_at: String,
    /// What the agent is currently working on and its state.
    pub active_task: String,
    /// Key decisions made with reasoning.
    pub decisions: Vec<String>,
    /// Files touched and why.
    pub files_modified: Vec<String>,
    /// Open questions, blockers, things left to do.
    pub unresolved: Vec<String>,
    /// Durable facts learned (user, project, environment).
    pub key_facts: Vec<String>,
    /// 2-3 sentence narrative summary for FIFO injection.
    pub summary: String,
}

/// Maximum number of rolling compaction summaries to keep.
const MAX_FIFO_SUMMARIES: usize = 2;

/// Directory for FIFO compaction summaries.
fn fifo_summary_dir(agent_id: &str) -> std::path::PathBuf {
    fifo_summary_dir_from_agent_base(&crate::config::app_config::agent_dir(agent_id))
}

fn fifo_summary_dir_from_agent_base(agent_base: &std::path::Path) -> std::path::PathBuf {
    agent_base.join("memory").join("compaction_summaries")
}

/// A stored FIFO summary entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FifoSummary {
    pub extracted_at: String,
    pub summary: String,
}

/// Save a FIFO summary, pruning oldest if over MAX_FIFO_SUMMARIES.
pub fn save_fifo_summary(agent_id: &str, result: &ExtractionResult) -> Result<(), String> {
    save_fifo_summary_to_dir(&fifo_summary_dir(agent_id), result)
}

fn save_fifo_summary_to_dir(
    dir: &std::path::Path,
    result: &ExtractionResult,
) -> Result<(), String> {
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create FIFO dir: {e}"))?;

    let entry = FifoSummary {
        extracted_at: result.extracted_at.clone(),
        summary: result.summary.clone(),
    };
    let filename = format!("{}.json", Utc::now().timestamp_millis());
    let path = dir.join(&filename);
    let json = serde_json::to_string_pretty(&entry)
        .map_err(|e| format!("Failed to serialize FIFO summary: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("Failed to write FIFO summary: {e}"))?;

    prune_fifo_summaries_in_dir(dir)?;
    Ok(())
}

/// Read all FIFO summaries, sorted oldest to newest.
pub fn read_fifo_summaries(agent_id: &str) -> Vec<FifoSummary> {
    read_fifo_summaries_from_dir(&fifo_summary_dir(agent_id))
}

fn read_fifo_summaries_from_dir(dir: &std::path::Path) -> Vec<FifoSummary> {
    let mut entries: Vec<(String, FifoSummary)> = match std::fs::read_dir(dir) {
        Ok(reader) => reader
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "json")
                    .unwrap_or(false)
            })
            .filter_map(|e| {
                let content = std::fs::read_to_string(e.path()).ok()?;
                let summary: FifoSummary = serde_json::from_str(&content).ok()?;
                Some((e.file_name().to_string_lossy().to_string(), summary))
            })
            .collect(),
        Err(_) => return vec![],
    };

    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.into_iter().map(|(_, s)| s).collect()
}

fn prune_fifo_summaries_in_dir(dir: &std::path::Path) -> Result<(), String> {
    let mut files: Vec<std::path::PathBuf> = match std::fs::read_dir(dir) {
        Ok(reader) => reader
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "json")
                    .unwrap_or(false)
            })
            .map(|e| e.path())
            .collect(),
        Err(_) => return Ok(()),
    };

    files.sort();
    while files.len() > MAX_FIFO_SUMMARIES {
        if let Some(oldest) = files.first() {
            let _ = std::fs::remove_file(oldest);
        }
        files.remove(0);
    }
    Ok(())
}

use crate::runtime::agent::ConversationMessage;

/// Strip markdown code fencing from LLM responses.
fn strip_markdown_fencing(text: &str) -> &str {
    let trimmed = text.trim();
    if trimmed.starts_with("```") {
        let after_fence = match trimmed.find('\n') {
            Some(pos) => &trimmed[pos + 1..],
            None => return trimmed,
        };
        if let Some(end) = after_fence.rfind("```") {
            return after_fence[..end].trim();
        }
        return after_fence.trim();
    }
    trimmed
}

/// Parse an LLM response into an ExtractionResult.
/// Handles bare JSON, markdown-fenced JSON, and returns Err for unparseable input.
/// Sets `extracted_at` to the current timestamp.
pub fn parse_extraction_response(response: &str) -> Result<ExtractionResult, String> {
    let raw = strip_markdown_fencing(response);
    let mut result: ExtractionResult =
        serde_json::from_str(raw).map_err(|e| format!("Failed to parse extraction JSON: {e}"))?;
    result.extracted_at = Utc::now().to_rfc3339();
    Ok(result)
}

/// Send conversation to an LLM for structured extraction.
///
/// Returns an `ExtractionResult` on success, or `Err` if the LLM call fails
/// or the response can't be parsed (caller should fall back to heuristic).
pub async fn llm_extract_context(
    messages: &[ConversationMessage],
    client: &reqwest::Client,
    endpoint: &str,
    api_key: Option<&str>,
    model: &str,
    summary_tokens: u32,
    keep_recent: usize,
) -> Result<ExtractionResult, String> {
    use crate::runtime::agent::MessageRole;

    // Adaptive truncation: recent messages full-length, older messages truncated.
    let recent_start = messages.len().saturating_sub(keep_recent);
    let conversation_text: String = messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let role = match m.role {
                MessageRole::System => "System",
                MessageRole::User => "User",
                MessageRole::Assistant => "Assistant",
                MessageRole::Tool => "Tool",
            };
            let content = if i >= recent_start {
                m.content.clone()
            } else {
                m.content.chars().take(500).collect::<String>()
            };
            format!("[{}]: {}", role, content)
        })
        .collect::<Vec<_>>()
        .join("\n");

    if conversation_text.is_empty() {
        return Err("No conversation content to extract from".to_string());
    }

    let prompt =
        crate::runtime::context::build_extraction_prompt(&conversation_text, summary_tokens);

    let provider = crate::providers::detect_and_build(endpoint, model, api_key, None, None)
        .map_err(|e| format!("Failed to build extraction provider: {e}"))?;
    let noop_callback = |_: crate::providers::types::StreamEvent| {};

    let response = crate::providers::send_provider_completion(
        provider.as_ref(),
        client,
        &prompt,
        None,
        summary_tokens,
        &noop_callback,
        "extraction",
    )
    .await
    .map_err(|e| format!("LLM extraction call failed: {e}"))?;

    parse_extraction_response(&response.content)
}

/// Parameters for the LLM extraction call, grouped to avoid too many function arguments.
pub struct ExtractionParams<'a> {
    pub client: &'a reqwest::Client,
    pub endpoint: &'a str,
    pub api_key: Option<&'a str>,
    pub model: &'a str,
    pub summary_tokens: u32,
    pub keep_recent: usize,
}

/// Check if pre-emptive extraction should run, and if so, execute it.
/// Called from the engine loop when context tokens are updated.
///
/// Returns the outcome of the extraction attempt.
pub async fn check_and_extract(
    engine: &MemoryEngine,
    agents: &Arc<RwLock<Vec<AgentSlot>>>,
    agent_id: &str,
    current_context_tokens: u64,
    extraction_threshold: u64,
    params: &ExtractionParams<'_>,
) -> Result<ExtractionOutcome, String> {
    if current_context_tokens < extraction_threshold {
        return Ok(ExtractionOutcome::NotNeeded);
    }

    // Check if we already extracted this cycle
    {
        let agents_r = agents.read().await;
        if let Some(agent) = agents_r.iter().find(|a| a.id == agent_id) {
            if agent.extraction_pending_done {
                return Ok(ExtractionOutcome::NotNeeded);
            }
        }
    }

    tracing::info!(
        agent_id,
        tokens = current_context_tokens,
        threshold = extraction_threshold,
        "Pre-emptive memory extraction triggered"
    );

    // Get conversation messages
    let messages = {
        let agents_r = agents.read().await;
        agents_r
            .iter()
            .find(|a| a.id == agent_id)
            .map(|a| a.conversation.messages.clone())
            .unwrap_or_default()
    };

    if messages.is_empty() {
        return Ok(ExtractionOutcome::NotNeeded);
    }

    let agent_ns = MemoryEngine::agent_namespace(agent_id);

    // Attempt LLM extraction, fall back to heuristic
    let extraction = match llm_extract_context(
        &messages,
        params.client,
        params.endpoint,
        params.api_key,
        params.model,
        params.summary_tokens,
        params.keep_recent,
    )
    .await
    {
        Ok(result) => {
            tracing::info!(agent_id, "LLM extraction succeeded");
            result
        }
        Err(e) => {
            tracing::warn!(
                agent_id,
                "LLM extraction failed, using heuristic fallback: {e}"
            );
            let (summary_text, active_work_pairs) = heuristic_extract_structured(&messages);
            let summary = if summary_text.is_empty() {
                // Ultimate fallback: raw text truncation
                let conversation_text: String = messages
                    .iter()
                    .map(|m| format!("{:?}: {}", m.role, m.content))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                build_summary(&conversation_text)
            } else {
                summary_text
            };
            let active_work_content = if active_work_pairs.is_empty() {
                let conversation_text: String = messages
                    .iter()
                    .map(|m| format!("{:?}: {}", m.role, m.content))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                heuristic_build_active_work(&conversation_text)
            } else {
                let mut content = String::from("# Active Work State\n\n");
                content.push_str("## Recent Context (structured extraction)\n\n");
                content.push_str(&active_work_pairs);
                content.push_str("\n\n---\n");
                content
                    .push_str("*Auto-generated by Holt pre-compaction extraction (heuristic).*\n");
                content
            };
            write_active_work(agent_id, &active_work_content).await;

            let summary_input = StoreInput {
                content: format!("[Compaction Summary] {}", summary),
                // Was MemoryCategory::State, which mapped to the same kind as Episode.
                category: MemoryCategory::Episode,
                importance: 0.9,
                source: MemorySource::AutoCaptured,
                agent_ns: agent_ns.clone(),
                tier: MemoryTier::Warm,
                tags: vec!["compaction".to_string()],
            };
            if let Err(e) = engine.store(summary_input).await {
                tracing::warn!(
                    agent_id,
                    "Compaction summary store failed, aborting compaction to allow retry: {e}"
                );
                // Do NOT set extraction_pending_done — leave it false so the
                // next cycle retries.  Do NOT evict conversation messages.
                return Ok(ExtractionOutcome::StoreFailed(format!(
                    "Compaction summary store failed: {e}"
                )));
            }

            // Store succeeded — safe to mark done and let eviction proceed.
            let mut agents_w = agents.write().await;
            if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
                agent.extraction_pending_done = true;
            }
            return Ok(ExtractionOutcome::Completed);
        }
    };

    // LLM extraction succeeded — write active_work.md
    let active_work_content = build_active_work_md(&extraction);
    write_active_work(agent_id, &active_work_content).await;

    // Save FIFO rolling summary
    if let Err(e) = save_fifo_summary(agent_id, &extraction) {
        tracing::warn!(agent_id, "Failed to save FIFO summary: {e}");
    }

    // Push raw extraction to bridge warm tier
    let extraction_json = serde_json::to_string(&extraction).unwrap_or_default();
    let source_tag = format!("{}/pre-compaction-extraction", agent_ns);
    let store_input = StoreInput {
        content: format!("[Pre-Compaction Extraction] {}", extraction_json),
        category: MemoryCategory::Episode,
        importance: 0.9,
        source: MemorySource::AutoCaptured,
        agent_ns: agent_ns.clone(),
        tier: MemoryTier::Warm,
        tags: vec!["extraction".to_string()],
    };
    if let Err(e) = engine.store(store_input).await {
        tracing::warn!(
            agent_id, source = %source_tag,
            "Extraction store failed, aborting compaction to allow retry: {e}"
        );
        // Do NOT set extraction_pending_done — leave it false so the
        // next cycle retries.  Do NOT evict conversation messages.
        return Ok(ExtractionOutcome::StoreFailed(format!(
            "Extraction store failed: {e}"
        )));
    }

    // Implicit memory feedback from compaction extraction
    if let Ok(extraction_val) = serde_json::from_str::<serde_json::Value>(&extraction_json) {
        if let Some(feedback_arr) = extraction_val
            .get("memory_feedback")
            .and_then(|v| v.as_array())
        {
            let mut feedback_count = 0u32;
            for fb in feedback_arr {
                let memory_id = fb.get("memory_id").and_then(|v| v.as_str()).unwrap_or("");
                let signal = fb.get("signal").and_then(|v| v.as_str()).unwrap_or("");
                if !memory_id.is_empty() && matches!(signal, "positive" | "negative") {
                    let source = format!("compaction:{}", agent_id);
                    let _ = engine.feedback(memory_id, signal, &source).await;
                    feedback_count += 1;
                }
            }
            if feedback_count > 0 {
                tracing::debug!(
                    agent_id,
                    count = feedback_count,
                    "Compaction implicit memory feedback sent"
                );
            }
        }
    }

    // Set extraction flag
    {
        let mut agents_w = agents.write().await;
        if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
            agent.extraction_pending_done = true;
        }
    }

    tracing::info!(agent_id, "Pre-emptive LLM extraction complete");
    Ok(ExtractionOutcome::Completed)
}

/// Reset extraction flag after compaction is detected.
/// Called when session.rs detects a context token drop > threshold.
pub async fn reset_extraction_flag(agents: &Arc<RwLock<Vec<AgentSlot>>>, agent_id: &str) {
    let mut agents_w = agents.write().await;
    if let Some(agent) = agents_w.iter_mut().find(|a| a.id == agent_id) {
        agent.extraction_pending_done = false;
    }
}

/// Build a brief summary from conversation text.
/// For now, uses a simple heuristic — takes the last few user/assistant exchanges.
/// Phase 3 enhancement: use the agent's own model for proper extraction.
fn build_summary(conversation_text: &str) -> String {
    // Take the last ~500 chars of conversation as a rough summary
    let text = conversation_text.trim();
    if text.len() <= 500 {
        text.to_string()
    } else {
        format!("...{}", &text[text.len() - 500..])
    }
}

/// Render an ExtractionResult into structured active_work.md markdown.
pub fn build_active_work_md(result: &ExtractionResult) -> String {
    let mut content = String::from("# Active Work State\n\n");
    content.push_str(&format!("*Extracted: {}*\n\n", result.extracted_at));

    content.push_str("## Current Task\n\n");
    content.push_str(&result.active_task);
    content.push_str("\n\n");

    if !result.decisions.is_empty() {
        content.push_str("## Key Decisions\n\n");
        for d in &result.decisions {
            content.push_str(&format!("- {}\n", d));
        }
        content.push('\n');
    }

    if !result.files_modified.is_empty() {
        content.push_str("## Files Modified\n\n");
        for f in &result.files_modified {
            content.push_str(&format!("- {}\n", f));
        }
        content.push('\n');
    }

    if !result.unresolved.is_empty() {
        content.push_str("## Unresolved\n\n");
        for u in &result.unresolved {
            content.push_str(&format!("- {}\n", u));
        }
        content.push('\n');
    }

    if !result.key_facts.is_empty() {
        content.push_str("## Key Facts\n\n");
        for f in &result.key_facts {
            content.push_str(&format!("- {}\n", f));
        }
        content.push('\n');
    }

    content.push_str("---\n");
    content.push_str("*Auto-generated by Holt LLM pre-compaction extraction. ");
    content.push_str("Read this to resume your task state after compaction.*\n");
    content
}

/// Heuristic fallback: build active_work.md from last 20 lines of conversation.
/// Used when LLM extraction fails.
fn heuristic_build_active_work(conversation_text: &str) -> String {
    let lines: Vec<&str> = conversation_text.lines().collect();
    let recent = if lines.len() > 20 {
        &lines[lines.len() - 20..]
    } else {
        &lines
    };

    let mut content = String::from("# Active Work State\n\n");
    content.push_str("## Last Context Before Compaction\n\n");
    content.push_str(&recent.join("\n"));
    content.push_str("\n\n---\n");
    content.push_str("*Auto-generated by Holt pre-compaction extraction.");
    content.push_str("Read this to resume your task state after compaction.*\n");
    content
}

/// Improved heuristic extraction that preserves structured content from
/// conversation messages. Prioritizes assistant messages (which contain
/// decisions, summaries, and task state) over raw text truncation.
///
/// Returns (summary, active_work_content).
fn heuristic_extract_structured(messages: &[ConversationMessage]) -> (String, String) {
    use crate::runtime::agent::MessageRole;

    if messages.is_empty() {
        return (String::new(), String::new());
    }

    // Collect assistant messages — most likely to contain decisions/summaries
    let assistant_msgs: Vec<&ConversationMessage> = messages
        .iter()
        .filter(|m| m.role == MessageRole::Assistant)
        .collect();

    // Build summary from last 5 assistant messages (or all messages if no assistants)
    let summary = if !assistant_msgs.is_empty() {
        assistant_msgs
            .iter()
            .rev()
            .take(5)
            .rev()
            .map(|m| m.content.chars().take(500).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n\n")
    } else {
        // Graceful degradation: use last 5 messages regardless of role
        messages
            .iter()
            .rev()
            .take(5)
            .rev()
            .map(|m| m.content.chars().take(500).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    // Build active_work from the most recent exchanges (last 6 messages = ~3 pairs)
    let recent_count = 6.min(messages.len());
    let recent_messages = &messages[messages.len() - recent_count..];
    let active_work = recent_messages
        .iter()
        .map(|m| {
            let role = match m.role {
                MessageRole::System => "System",
                MessageRole::User => "User",
                MessageRole::Assistant => "Assistant",
                MessageRole::Tool => "Tool",
            };
            format!(
                "[{}]: {}",
                role,
                m.content.chars().take(500).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    (summary, active_work)
}

/// Write active_work.md to the agent's persona folder.
/// Auto-update marker used to separate human-written content from auto-generated content.
const ACTIVE_WORK_MARKER: &str = "<!-- AUTO-UPDATED";

/// Write active_work.md to the agent's persona folder.
/// Preserves everything above the `<!-- AUTO-UPDATED -->` marker.
/// If no marker exists, appends the marker and auto-generated content.
pub async fn write_active_work(agent_id: &str, content: &str) {
    let base = crate::config::app_config::agent_dir(agent_id).join("persona");

    if let Err(e) = tokio::fs::create_dir_all(&base).await {
        tracing::warn!(agent_id, "Failed to create persona dir: {e}");
        return;
    }

    let path = base.join("ACTIVE_WORK.md");
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    // Read existing content and preserve everything above the marker
    let existing = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let preserved = match existing.find(ACTIVE_WORK_MARKER) {
        Some(pos) => existing[..pos].trim_end().to_string(),
        None if !existing.is_empty() => existing.trim_end().to_string(),
        None => String::new(),
    };

    let marker_line = format!("{}: {} -->", ACTIVE_WORK_MARKER, timestamp);

    let new_content = if preserved.is_empty() {
        format!("{}\n\n{}", marker_line, content)
    } else {
        format!("{}\n\n{}\n\n{}", preserved, marker_line, content)
    };

    match tokio::fs::write(&path, new_content).await {
        Ok(_) => tracing::info!(agent_id, path = %path.display(), "Updated ACTIVE_WORK.md"),
        Err(e) => tracing::warn!(agent_id, "Failed to write ACTIVE_WORK.md: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn extraction_result_survives_json_roundtrip() {
        let result = ExtractionResult {
            extracted_at: "2026-03-29T12:00:00Z".to_string(),
            active_task: "Implementing pre-compaction extraction".to_string(),
            decisions: vec!["Use LLM for extraction".to_string()],
            files_modified: vec!["memory/compaction.rs — added ExtractionResult".to_string()],
            unresolved: vec!["Hillock push not yet wired".to_string()],
            key_facts: vec!["Project uses memory-bridge at port 8081".to_string()],
            summary: "Working on Phase 3.1. Struct defined, tests next.".to_string(),
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: ExtractionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.extracted_at, result.extracted_at);
        assert_eq!(parsed.active_task, result.active_task);
        assert_eq!(parsed.decisions, result.decisions);
        assert_eq!(parsed.files_modified, result.files_modified);
        assert_eq!(parsed.unresolved, result.unresolved);
        assert_eq!(parsed.key_facts, result.key_facts);
        assert_eq!(parsed.summary, result.summary);
    }

    #[test]
    fn parse_extraction_handles_clean_json() {
        let json = r#"{"active_task":"test","decisions":[],"files_modified":[],"unresolved":[],"key_facts":[],"summary":"ok"}"#;
        let result = parse_extraction_response(json).unwrap();
        assert_eq!(result.active_task, "test");
        assert_eq!(result.summary, "ok");
        assert!(
            !result.extracted_at.is_empty(),
            "extracted_at should be set"
        );
    }

    #[test]
    fn parse_extraction_handles_fenced_json() {
        let response = "```json\n{\"active_task\":\"test\",\"decisions\":[],\"files_modified\":[],\"unresolved\":[],\"key_facts\":[],\"summary\":\"ok\"}\n```";
        let result = parse_extraction_response(response).unwrap();
        assert_eq!(result.active_task, "test");
    }

    #[test]
    fn parse_extraction_handles_bare_backtick_fence() {
        let response = "```\n{\"active_task\":\"test\",\"decisions\":[],\"files_modified\":[],\"unresolved\":[],\"key_facts\":[],\"summary\":\"ok\"}\n```";
        let result = parse_extraction_response(response).unwrap();
        assert_eq!(result.active_task, "test");
    }

    #[test]
    fn parse_extraction_rejects_garbage() {
        let result = parse_extraction_response("This is not JSON at all");
        assert!(result.is_err());
    }

    #[test]
    fn fifo_keeps_only_newest_two_summaries() {
        let dir = tempdir().unwrap();
        let fifo_dir = fifo_summary_dir_from_agent_base(dir.path());

        for i in 0..3 {
            let result = ExtractionResult {
                extracted_at: format!("2026-03-29T12:0{}:00Z", i),
                active_task: format!("Task {}", i),
                decisions: vec![],
                files_modified: vec![],
                unresolved: vec![],
                key_facts: vec![],
                summary: format!("Summary {}", i),
            };
            save_fifo_summary_to_dir(&fifo_dir, &result).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let summaries = read_fifo_summaries_from_dir(&fifo_dir);
        assert_eq!(summaries.len(), 2);
        assert!(summaries[0].summary.contains("Summary 1"));
        assert!(summaries[1].summary.contains("Summary 2"));
    }

    #[test]
    fn active_work_md_includes_all_sections() {
        let result = ExtractionResult {
            extracted_at: "2026-03-29T12:00:00Z".to_string(),
            active_task: "Building extraction".to_string(),
            decisions: vec!["Use LLM".to_string()],
            files_modified: vec!["compaction.rs".to_string()],
            unresolved: vec!["Hillock push".to_string()],
            key_facts: vec!["Uses port 8081".to_string()],
            summary: "Working on Phase 3.1.".to_string(),
        };

        let md = build_active_work_md(&result);
        assert!(md.contains("# Active Work State"));
        assert!(md.contains("2026-03-29T12:00:00Z"));
        assert!(md.contains("Building extraction"));
        assert!(md.contains("Use LLM"));
        assert!(md.contains("compaction.rs"));
        assert!(md.contains("Hillock push"));
        assert!(md.contains("Uses port 8081"));
    }

    #[test]
    fn active_work_md_omits_empty_sections() {
        let result = ExtractionResult {
            extracted_at: "2026-03-29T12:00:00Z".to_string(),
            active_task: "Building extraction".to_string(),
            decisions: vec![],
            files_modified: vec![],
            unresolved: vec![],
            key_facts: vec![],
            summary: "Summary only.".to_string(),
        };

        let md = build_active_work_md(&result);
        assert!(md.contains("Building extraction"));
        assert!(!md.contains("Key Decisions"));
        assert!(!md.contains("Files Modified"));
        assert!(!md.contains("Unresolved"));
        assert!(!md.contains("Key Facts"));
    }

    #[test]
    fn fifo_read_empty_returns_empty() {
        let dir = tempdir().unwrap();
        let fifo_dir = fifo_summary_dir_from_agent_base(dir.path());
        let summaries = read_fifo_summaries_from_dir(&fifo_dir);
        assert!(summaries.is_empty());
    }

    // ── 5D.2: ExtractionOutcome enum ────────────────────────────────────

    #[test]
    fn extraction_outcome_distinguishes_not_needed_from_store_failed() {
        let not_needed = ExtractionOutcome::NotNeeded;
        let store_failed = ExtractionOutcome::StoreFailed("DB unreachable".to_string());
        let completed = ExtractionOutcome::Completed;

        assert_ne!(not_needed, store_failed);
        assert_ne!(not_needed, completed);
        assert_ne!(store_failed, completed);
    }

    #[test]
    fn extraction_outcome_store_failed_carries_reason() {
        let outcome = ExtractionOutcome::StoreFailed("connection refused".to_string());
        if let ExtractionOutcome::StoreFailed(reason) = outcome {
            assert!(reason.contains("connection refused"));
        } else {
            panic!("Expected StoreFailed variant");
        }
    }

    // ── 5D.1: Heuristic fallback preserves structured content ─────────

    #[test]
    fn heuristic_fallback_preserves_assistant_messages() {
        use crate::runtime::agent::MessageRole;

        let messages = vec![
            super::ConversationMessage {
                role: MessageRole::User,
                content: "What should we do about the config?".to_string(),
                content_blocks: None,
                timestamp: chrono::Utc::now(),
                token_estimate: None,
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            },
            super::ConversationMessage {
                role: MessageRole::Assistant,
                content: "Decision: We should use serde(default) for backward compatibility. I've updated config.rs to add the new field.".to_string(),
                content_blocks: None,
                timestamp: chrono::Utc::now(),
                token_estimate: None,
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            },
            super::ConversationMessage {
                role: MessageRole::User,
                content: "Looks good".to_string(),
                content_blocks: None,
                timestamp: chrono::Utc::now(),
                token_estimate: None,
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            },
            super::ConversationMessage {
                role: MessageRole::Assistant,
                content: "Now working on the migration path for existing configs.".to_string(),
                content_blocks: None,
                timestamp: chrono::Utc::now(),
                token_estimate: None,
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            },
        ];

        let (summary, active_work) = heuristic_extract_structured(&messages);

        // Should preserve the decision from assistant messages
        assert!(
            summary.contains("serde(default)"),
            "summary should contain decision content"
        );
        assert!(
            summary.contains("migration path"),
            "summary should contain latest assistant message"
        );
        // active_work should contain the recent exchange
        assert!(!active_work.is_empty(), "active_work should not be empty");
    }

    #[test]
    fn heuristic_fallback_handles_empty_conversation() {
        let messages: Vec<super::ConversationMessage> = vec![];
        let (summary, active_work) = heuristic_extract_structured(&messages);
        assert!(summary.is_empty() || summary.contains("No conversation"));
        assert!(active_work.is_empty() || active_work.contains("No conversation"));
    }

    #[test]
    fn heuristic_fallback_handles_all_user_messages() {
        use crate::runtime::agent::MessageRole;

        let messages: Vec<super::ConversationMessage> = (0..5)
            .map(|i| super::ConversationMessage {
                role: MessageRole::User,
                content: format!("User message {}", i),
                content_blocks: None,
                timestamp: chrono::Utc::now(),
                token_estimate: None,
                tool_calls: None,
                tool_call_id: None,
                outcome_tag: None,
                thinking_content: None,
                responses_phase: None,
                reasoning_items: None,
            })
            .collect();

        let (summary, active_work) = heuristic_extract_structured(&messages);
        // Should degrade gracefully — use whatever messages are available
        assert!(
            !summary.is_empty(),
            "should produce some summary from user messages"
        );
        assert!(
            !active_work.is_empty(),
            "should produce some active work from user messages"
        );
    }
}
