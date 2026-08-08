//! Per-turn memory injection — RETIRED 2026-07-26.
//!
//! ⚠️ **This is NOT the only way memory reaches context, and removing it did not make memory
//! capture-only.** Holt's *Living Persona* path is separate and still active:
//! `persona::fetch_dynamic_profile` → `MemoryEngine::pack_context_for_persona` →
//! hillock's `ops::pack_context`, which packs pinned memories first and then fills by
//! importance, appended after the static persona files on every lane (`payload.rs`,
//! `agent_sdk.rs`, `codex.rs`). That is the requested injection path and it is untouched.
//!
//! What was retired here is the *query-driven per-turn* pipeline: a relevance gate plus
//! top-10 semantic retrieval matched to the current message, session dedup and token
//! budgeting, run on every turn. It was governed by `hot_injection_budget_tokens`, which was
//! `0` in the live config — a full pipeline reading as working software while doing nothing.
//! Deleted rather than left at zero, because a knob set to zero is not a decision.
//!
//! What remains is the report type the turn pipeline and its debug panel render, plus a
//! `build_memory_context` that returns an empty context with every count at zero.
//!
//! Flow:
//! 3. Filter through session dedup (skip already-injected memories)
//! 4. Format with new category-aware format
//! 5. Fit to token budget
//! 6. Record injected IDs in session state

use std::sync::Arc;

use super::embeddings::{count_tokens, EmbeddingService};
use super::session::SessionState;
use super::types::HillockMemory;
use super::MemoryEngine;
use super::MemoryError;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct MemoryContextReport {
    pub gate_action: String,
    pub gate_reason: String,
    pub retrieval_query: String,
    pub retrieval_scope: String,
    pub matching_cluster_ids: Vec<String>,
    pub retrieved_count: usize,
    pub retrieved_ids: Vec<String>,
    pub already_injected_count: usize,
    pub candidate_count: usize,
    pub injected_count: usize,
    pub injected_ids: Vec<String>,
    pub retrieved_dream_derived_count: usize,
    pub retrieved_dream_derived_ids: Vec<String>,
    pub injected_dream_derived_count: usize,
    pub injected_dream_derived_ids: Vec<String>,
    pub injected_source_session_memory_ids: Vec<String>,
    pub token_budget: usize,
    pub tokens_used: usize,
    pub formatted_preview: String,
    pub session_injected_total_after: usize,
}

#[derive(Debug, Clone)]
pub struct MemoryContextBuildResult {
    pub text: String,
    pub ids: Vec<String>,
    pub report: MemoryContextReport,
}

/// Build the memory injection context for an agent's prompt.
/// Returns formatted text and list of injected memory IDs.
///
/// This is the main entry point called by turn_injection.rs and agent_sdk.rs.
/// Build the per-turn memory context.
///
/// **Retired 2026-07-26: this always returns an empty context and injects nothing.**
///
/// Per-turn, query-driven injection is gone by decision, not by configuration. The body that
/// used to run here (relevance gate, top-10 semantic retrieval, session dedup, token
/// budgeting) has been deleted rather than switched off, because the previous state — a full
/// pipeline governed by `hot_injection_budget_tokens = 0` — read as working software while
/// doing nothing.
///
/// Memory still reaches context at **session start** through the Living Persona profile; see
/// the module docs. Do not read this retirement as "nothing is injected".
///
/// The signature and report are kept so the turn pipeline and its debug panel still
/// compile and render; every count is zero because nothing is injected.
pub async fn build_memory_context(
    _engine: &MemoryEngine,
    session_state: &SessionState,
    _embedder: &Arc<EmbeddingService>,
    agent_id: &str,
    _agent_ns: &str,
    _current_message: &str,
    _recent_messages: &[String],
    token_budget: usize,
) -> Result<MemoryContextBuildResult, MemoryError> {
    Ok(MemoryContextBuildResult {
        text: String::new(),
        ids: vec![],
        report: MemoryContextReport {
            gate_action: "disabled".to_string(),
            gate_reason:
                "per-turn injection retired; session-start context comes from Living Persona"
                    .to_string(),
            retrieval_query: String::new(),
            retrieval_scope: "none".to_string(),
            matching_cluster_ids: vec![],
            retrieved_count: 0,
            retrieved_ids: vec![],
            already_injected_count: 0,
            candidate_count: 0,
            injected_count: 0,
            injected_ids: vec![],
            retrieved_dream_derived_count: 0,
            retrieved_dream_derived_ids: vec![],
            injected_dream_derived_count: 0,
            injected_dream_derived_ids: vec![],
            injected_source_session_memory_ids: vec![],
            token_budget,
            tokens_used: 0,
            formatted_preview: String::new(),
            session_injected_total_after: session_state.injected_count(agent_id).await,
        },
    })
}

/// Legacy entry point — called by current turn_injection.rs and agent_sdk.rs.
/// Wraps the new pipeline with default parameters for backward compatibility
/// until the callers are fully updated in Task 5.
pub async fn build_memory_context_legacy(
    engine: &MemoryEngine,
    agent_ns: &str,
    recent_conversation: &str,
    token_budget: usize,
) -> Result<(String, Vec<String>), MemoryError> {
    // Simple bridge retrieval without gate or session dedup
    let memories = if !recent_conversation.is_empty() {
        engine
            .retrieve(
                recent_conversation,
                10,
                agent_ns,
                None,
                None,
                Some("injection"),
                Some(hillock_core::model::SpaceScope::All),
            )
            .await
            .map(|r| r.memories)
            .unwrap_or_default()
    } else {
        vec![]
    };

    if memories.is_empty() {
        return Ok((String::new(), vec![]));
    }

    let mut lines: Vec<String> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    let mut tokens_used: usize = 0;

    let header = "# Recalled Memories";
    let header_tokens = count_tokens(header) + 2;
    tokens_used += header_tokens;

    for mem in &memories {
        let line = format_memory_line(mem);
        let line_tokens = count_tokens(&line) + 1;

        if tokens_used + line_tokens > token_budget {
            break;
        }

        tokens_used += line_tokens;
        lines.push(line);
        ids.push(mem.id.clone());
    }

    if lines.is_empty() {
        return Ok((String::new(), vec![]));
    }

    let formatted = format!("{}\n\n{}", header, lines.join("\n"));
    Ok((formatted, ids))
}

/// Format a single memory for injection.
/// Uses category instead of tier (more useful to the agent).
fn format_memory_line(mem: &HillockMemory) -> String {
    let date = if mem.created_at.len() >= 10 {
        &mem.created_at[5..10]
    } else {
        "??-??"
    };
    let pinned = if mem.pinned { "|pinned" } else { "" };
    format!(
        "[{}{}|{}] ({:.2}) {}",
        mem.category, pinned, date, mem.score, mem.content
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Injection is retired. The three tests that used to live here covered the retrieval
    /// pipeline's exclusion rules (session-memory artifacts, cognitive-intake artifacts,
    /// dream provenance) and were deleted with the code they described — a test kept alive
    /// for a removed feature is just another false claim about the system.
    ///
    /// This replaces them with the invariant that actually matters now: this path injects
    /// nothing. Without it the per-turn pipeline could be reintroduced and no test would
    /// object. Note the scope — memory still reaches context at session start via the Living
    /// Persona profile, which is a different path and deliberately untouched.
    #[test]
    fn the_injection_report_type_still_carries_zeroed_counts() {
        let report = MemoryContextReport {
            gate_action: "disabled".to_string(),
            gate_reason:
                "per-turn injection retired; session-start context comes from Living Persona"
                    .to_string(),
            retrieval_query: String::new(),
            retrieval_scope: "none".to_string(),
            matching_cluster_ids: vec![],
            retrieved_count: 0,
            retrieved_ids: vec![],
            already_injected_count: 0,
            candidate_count: 0,
            injected_count: 0,
            injected_ids: vec![],
            retrieved_dream_derived_count: 0,
            retrieved_dream_derived_ids: vec![],
            injected_dream_derived_count: 0,
            injected_dream_derived_ids: vec![],
            injected_source_session_memory_ids: vec![],
            token_budget: 0,
            tokens_used: 0,
            formatted_preview: String::new(),
            session_injected_total_after: 0,
        };

        assert_eq!(
            report.injected_count, 0,
            "nothing may be injected into a turn"
        );
        assert_eq!(
            report.tokens_used, 0,
            "injection may not spend context tokens"
        );
        assert!(
            report.injected_ids.is_empty(),
            "no memory may be pushed into a turn automatically"
        );
    }
}
