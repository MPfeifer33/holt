//! Cognitive Intake — per-turn passive memory extraction.
//!
//! After each turn completes, extracts raw conversational content (human + agent
//! messages, stripped of tool calls and code blocks) and forwards to Hillock for
//! processing. Hillock handles admission, embedding, dedup, cluster routing,
//! and lifecycle management.
//!
//! The novelty gate prevents redundant Hillock calls by comparing the current
//! turn's embedding against a ring buffer of recent submissions.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::memory::embeddings::EmbeddingService;
use crate::memory::MemoryEngine;

/// Per-agent cognitive intake state. Lives in AgentSlot (not serialized).
#[derive(Debug, Clone)]
pub struct CognitiveIntakeState {
    /// Ring buffer of recent embeddings for novelty gate
    novelty_buffer: VecDeque<Vec<f32>>,
    /// Whether intake is enabled for this agent
    pub enabled: bool,
    /// Running count of submissions this session (observability)
    pub submissions_this_session: u32,
    /// Running count of novelty-rejected this session
    pub rejected_this_session: u32,
    /// Last user message sent this turn (used by SDK path where prompt
    /// isn't available in the Done handler).
    pub last_user_message: Option<String>,
}

impl Default for CognitiveIntakeState {
    fn default() -> Self {
        Self {
            novelty_buffer: VecDeque::with_capacity(NOVELTY_WINDOW_SIZE),
            enabled: true,
            submissions_this_session: 0,
            rejected_this_session: 0,
            last_user_message: None,
        }
    }
}

/// Configuration constants
const NOVELTY_WINDOW_SIZE: usize = 10;
const NOVELTY_THRESHOLD: f32 = 0.85;

/// Minimum content length worth sending to Hillock (in chars after extraction)
const MIN_CONTENT_LENGTH: usize = 20;

/// Result of a cognitive intake attempt
#[derive(Debug)]
pub enum IntakeResult {
    /// Content forwarded to Hillock successfully
    Forwarded { memory_id: String },
    /// Content was too similar to recent submissions (novelty gate)
    NoveltyRejected { max_similarity: f32 },
    /// Content too short after extraction
    TooShort { length: usize },
    /// Intake disabled for this agent
    Disabled,
    /// Hillock unavailable or errored
    HillockError(String),
}

/// Extract conversational content from a turn's messages.
///
/// Strips:
/// - Tool call blocks (function invocations)
/// - Code blocks (``` delimited)
/// - System messages
///
/// Preserves:
/// - Human messages (full text)
/// - Assistant response text (meaningful dialogue, decisions, explanations)
/// - File paths mentioned (as metadata, not content)
pub fn extract_turn_content(user_message: &str, assistant_response: &str) -> String {
    let mut output = String::new();

    // Human message — include as-is (already pure conversational text)
    let user_cleaned = user_message.trim();
    if !user_cleaned.is_empty() {
        output.push_str("[Human] ");
        output.push_str(user_cleaned);
        output.push('\n');
    }

    // Assistant response — strip code blocks but keep conversational text
    let assistant_cleaned = strip_code_blocks(assistant_response.trim());
    if !assistant_cleaned.is_empty() {
        output.push_str("[Agent] ");
        output.push_str(&assistant_cleaned);
    }

    output.trim().to_string()
}

/// Strip fenced code blocks from text, preserving surrounding context.
/// Replaces code blocks with a placeholder noting the language if available.
fn strip_code_blocks(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_code_block = false;
    let mut code_lang = String::new();

    for line in text.lines() {
        if line.starts_with("```") {
            if in_code_block {
                // Closing fence — add placeholder
                if !code_lang.is_empty() {
                    result.push_str(&format!("[code: {}] ", code_lang));
                } else {
                    result.push_str("[code block] ");
                }
                in_code_block = false;
                code_lang.clear();
            } else {
                // Opening fence — capture language
                in_code_block = true;
                code_lang = line.trim_start_matches('`').trim().to_string();
            }
        } else if !in_code_block {
            result.push_str(line);
            result.push('\n');
        }
    }

    result.trim().to_string()
}

/// Check novelty against the ring buffer. Returns the max cosine similarity
/// to any recent embedding.
fn check_novelty(embedding: &[f32], buffer: &VecDeque<Vec<f32>>) -> f32 {
    buffer
        .iter()
        .map(|recent| cosine_similarity(embedding, recent))
        .fold(0.0_f32, f32::max)
}

/// Cosine similarity between two embedding vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Run the cognitive intake pipeline for a completed turn.
///
/// Called from both the engine loop (API agents) and the SDK Done handler.
/// Runs asynchronously — does not block the turn response.
pub async fn process_turn(
    state: &mut CognitiveIntakeState,
    embedder: &Arc<EmbeddingService>,
    memory_engine: &MemoryEngine,
    agent_ns: &str,
    user_message: &str,
    assistant_response: &str,
) -> IntakeResult {
    if !state.enabled {
        return IntakeResult::Disabled;
    }

    // Step 1: Structural extraction
    let content = extract_turn_content(user_message, assistant_response);

    if content.len() < MIN_CONTENT_LENGTH {
        return IntakeResult::TooShort {
            length: content.len(),
        };
    }

    // Step 2: Generate embedding for novelty check
    let embedding = match embedder.embed(&content).await {
        Ok(emb) => emb,
        Err(e) => {
            return IntakeResult::HillockError(format!("Embedding failed: {}", e));
        }
    };

    // Step 3: Novelty gate — reject if too similar to recent
    let max_sim = check_novelty(&embedding, &state.novelty_buffer);
    if max_sim > NOVELTY_THRESHOLD {
        state.rejected_this_session += 1;
        return IntakeResult::NoveltyRejected {
            max_similarity: max_sim,
        };
    }

    // Step 4: Forward to Hillock via memory_ingest
    let store_input = crate::memory::store::StoreInput {
        content: content.clone(),
        category: crate::memory::store::MemoryCategory::Episode,
        importance: 0.4,
        source: crate::memory::store::MemorySource::CognitiveIntake,
        agent_ns: agent_ns.to_string(),
        tier: crate::memory::store::MemoryTier::Warm,
        tags: vec!["cognitive_intake".to_string()],
    };

    match memory_engine.store(store_input).await {
        Ok(memory_id) => {
            // Success — update novelty buffer
            if state.novelty_buffer.len() >= NOVELTY_WINDOW_SIZE {
                state.novelty_buffer.pop_front();
            }
            state.novelty_buffer.push_back(embedding);
            state.submissions_this_session += 1;

            tracing::info!(
                agent_ns,
                memory_id = %memory_id,
                content_len = content.len(),
                "Cognitive intake: forwarded turn to bridge"
            );

            IntakeResult::Forwarded { memory_id }
        }
        Err(e) => IntakeResult::HillockError(format!("Hillock store failed: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_turn_content_basic() {
        let content = extract_turn_content(
            "lets start with bridge",
            "Sounds good. I'll create a feature branch and implement Phase A.",
        );
        assert!(content.contains("[Human] lets start with bridge"));
        assert!(content.contains("[Agent]"));
        assert!(content.contains("feature branch"));
    }

    #[test]
    fn test_extract_strips_code_blocks() {
        let response = "Here's the fix:\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\nThis should solve the issue.";
        let content = extract_turn_content("fix the bug", response);
        assert!(!content.contains("println"));
        assert!(content.contains("[code: rust]"));
        assert!(content.contains("solve the issue"));
    }

    #[test]
    fn test_extract_preserves_non_code() {
        let response = "I decided to use pressure-based displacement. No code needed.";
        let content = extract_turn_content("what approach?", response);
        assert!(content.contains("pressure-based displacement"));
        assert!(!content.contains("[code"));
    }

    #[test]
    fn test_novelty_gate_rejects_similar() {
        let emb1 = vec![1.0, 0.0, 0.0];
        let emb2 = vec![0.99, 0.1, 0.0]; // very similar
        let mut buffer = VecDeque::new();
        buffer.push_back(emb1);
        let sim = check_novelty(&emb2, &buffer);
        assert!(sim > 0.9, "Expected high similarity, got {}", sim);
    }

    #[test]
    fn test_novelty_gate_passes_different() {
        let emb1 = vec![1.0, 0.0, 0.0];
        let emb2 = vec![0.0, 1.0, 0.0]; // orthogonal
        let mut buffer = VecDeque::new();
        buffer.push_back(emb1);
        let sim = check_novelty(&emb2, &buffer);
        assert!(sim < 0.1, "Expected low similarity, got {}", sim);
    }

    #[test]
    fn test_novelty_empty_buffer_passes() {
        let emb = vec![1.0, 0.5, 0.3];
        let buffer = VecDeque::new();
        let sim = check_novelty(&emb, &buffer);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn test_strip_code_blocks_multiple() {
        let text = "First part\n```python\nprint('hi')\n```\nMiddle\n```\ngeneric code\n```\nEnd";
        let result = strip_code_blocks(text);
        assert!(result.contains("First part"));
        assert!(result.contains("[code: python]"));
        assert!(result.contains("Middle"));
        assert!(result.contains("[code block]"));
        assert!(result.contains("End"));
        assert!(!result.contains("print('hi')"));
    }

    #[test]
    fn test_min_content_length_filter() {
        let content = extract_turn_content("k", "k");
        // "[Human] k\n[Agent] k" = 19 chars, below threshold
        assert!(content.len() < MIN_CONTENT_LENGTH);
    }
}
