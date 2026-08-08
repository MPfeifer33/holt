use serde::{Deserialize, Serialize};

use crate::config::app_config::ContextBlock;

/// Token budget allocation for an agent's context window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBudget {
    /// Model's total context window (e.g., 200000)
    pub total: u32,
    /// Tokens reserved for model output (e.g., 8000)
    pub reserved_for_response: u32,
    /// Max tokens for system prompt + tools + context (e.g., 16000)
    pub pinned_top_max: u32,
    /// Max tokens for recent tool results + current message (e.g., 12000)
    pub pinned_bottom_max: u32,
    /// Remaining = total - response - top - bottom (computed)
    pub sliding_window: u32,
    /// Characters per token estimate (default 4.0)
    pub char_ratio: f32,
}

impl Default for ContextBudget {
    fn default() -> Self {
        let total = 125_000u32;
        let reserved = 35_000u32;
        let top = 16_000u32;
        let bottom = 12_000u32;
        Self {
            total,
            reserved_for_response: reserved,
            pinned_top_max: top,
            pinned_bottom_max: bottom,
            sliding_window: total.saturating_sub(reserved + top + bottom),
            char_ratio: 4.0,
        }
    }
}

impl ContextBudget {
    /// Create a ContextBudget from app config.
    pub fn from_config(config: &ContextBlock, model_context_size: u32) -> Self {
        let total = model_context_size;
        let reserved = config.response_reserve_tokens;
        let top = config.pinned_top_max_tokens;
        let bottom = config.pinned_bottom_max_tokens;
        Self {
            total,
            reserved_for_response: reserved,
            pinned_top_max: top,
            pinned_bottom_max: bottom,
            sliding_window: total.saturating_sub(reserved + top + bottom),
            char_ratio: config.char_ratio,
        }
    }

    /// Estimate token count from text using character ratio.
    pub fn estimate_tokens(text: &str, char_ratio: f32) -> u32 {
        (text.chars().count() as f32 / char_ratio).ceil() as u32
    }

    /// Available tokens for input (total minus response reserve).
    pub fn available_input_tokens(&self) -> u32 {
        self.total.saturating_sub(self.reserved_for_response)
    }

    /// Check if the given token count exceeds the compaction threshold.
    pub fn needs_compaction(&self, used_tokens: u32, threshold_percent: u32) -> bool {
        let threshold =
            (self.available_input_tokens() as f64 * threshold_percent as f64 / 100.0) as u32;
        used_tokens >= threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_budget() {
        let budget = ContextBudget::default();
        assert_eq!(budget.total, 125_000);
        assert_eq!(budget.reserved_for_response, 35_000);
        assert_eq!(budget.sliding_window, 125_000 - 35_000 - 16_000 - 12_000);
    }

    #[test]
    fn test_token_estimation() {
        // 100 ASCII chars at ratio 4.0 = 25 tokens
        let text = "a".repeat(100);
        assert_eq!(ContextBudget::estimate_tokens(&text, 4.0), 25);

        // Empty string = 0 tokens
        assert_eq!(ContextBudget::estimate_tokens("", 4.0), 0);

        // Multi-byte: 10 emoji chars at ratio 4.0 = 3 tokens (ceil)
        let emoji = "🎉".repeat(10);
        assert_eq!(ContextBudget::estimate_tokens(&emoji, 4.0), 3);
    }

    #[test]
    fn test_needs_compaction() {
        let budget = ContextBudget::default();
        let available = budget.available_input_tokens(); // 90000
        let threshold_100 = (available as f64 * 1.0) as u32; // 90000

        assert!(!budget.needs_compaction(threshold_100 - 1, 100));
        assert!(budget.needs_compaction(threshold_100, 100));
        assert!(budget.needs_compaction(threshold_100 + 1000, 100));
    }
}
