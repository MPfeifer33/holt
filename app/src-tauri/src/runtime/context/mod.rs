pub mod budget;
pub mod model_sizes;
pub mod payload;
pub mod prompts;
pub mod truncation;

// Re-export all public items at the module root for API compatibility.
pub use budget::ContextBudget;
pub use model_sizes::{
    known_context_size, provider_default_context_size, resolve_context_size,
    resolve_context_size_with_endpoint,
};
pub use payload::{build_context_payload, message_to_json, ContextPayload};
pub use prompts::{
    build_compaction_prompt, build_extraction_prompt, build_self_summary_prompt, compact_messages,
    rule_based_summary,
};
pub use truncation::truncate_tool_result;
