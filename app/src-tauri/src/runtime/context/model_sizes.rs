/// Lookup table of known model context sizes (Tier 2 detection).
///
/// Matches on model name substrings. More specific matches are checked first
/// to avoid false positives (e.g. "grok-4" before generic "grok").
pub fn known_context_size(model_name: &str) -> Option<u32> {
    let name = model_name.to_lowercase();

    // --- Anthropic ---
    if name.contains("claude") {
        // All Claude 3+ models (opus, sonnet, haiku) are 200k
        return Some(200_000);
    }

    // --- Google ---
    if name.contains("gemini") {
        if name.contains("2.5") {
            return Some(1_000_000);
        }
        if name.contains("2.0") || name.contains("flash") {
            return Some(1_000_000);
        }
        // Gemini 1.5 pro/flash
        return Some(1_000_000);
    }

    // --- OpenAI ---
    if name.contains("o3") || name.contains("o4") {
        return Some(200_000);
    }
    if name.contains("o1") {
        return Some(200_000);
    }
    if name.contains("gpt-4.1") || name.contains("gpt-4-1") {
        return Some(1_000_000);
    }
    if name.contains("gpt-4o") {
        return Some(128_000);
    }
    if name.contains("gpt-4") {
        return Some(128_000);
    }

    // --- xAI ---
    if name.contains("grok") {
        if name.contains("grok-4") {
            return Some(2_000_000);
        }
        return Some(131_072);
    }

    // --- DeepSeek ---
    if name.contains("deepseek") {
        if name.contains("v3") || name.contains("r1") {
            return Some(128_000);
        }
        return Some(128_000);
    }

    // --- Qwen ---
    if name.contains("qwen") {
        if name.contains("coder") {
            return Some(131_072);
        }
        return Some(128_000);
    }

    // --- Meta ---
    if name.contains("llama") {
        return Some(128_000);
    }

    // --- Mistral ---
    if name.contains("mistral") || name.contains("codestral") {
        return Some(128_000);
    }

    None
}

/// Endpoint-based fallback when model name isn't recognized.
/// Returns a conservative default for the provider so we don't undersize
/// the context budget for unknown/custom model names.
pub fn provider_default_context_size(endpoint: &str) -> Option<u32> {
    let ep = endpoint.to_lowercase();
    if ep.contains("anthropic") {
        return Some(200_000);
    }
    if ep.contains("generativelanguage.googleapis.com") {
        return Some(1_000_000);
    }
    if ep.contains("api.openai.com") {
        return Some(128_000);
    }
    if ep.contains("api.x.ai") {
        return Some(131_072);
    }
    if ep.contains("api.deepseek.com") {
        return Some(128_000);
    }
    if ep.contains("api.mistral.ai") {
        return Some(128_000);
    }
    None
}

/// Resolve context window size for an agent.
///
/// Priority chain (first match wins):
///   1. Runtime cloud API probe (most accurate)
///   2. Per-agent user override (`[limits] context_tokens`)
///   3. Model name lookup table (`known_context_size`)
///   4. Endpoint/provider fallback (`provider_default_context_size`)
///   5. Global default (125_000)
pub fn resolve_context_size(
    agent_context_tokens: Option<u32>,
    model_name: &str,
    global_default: u32,
    probed_context: Option<u32>,
) -> u32 {
    probed_context
        .or(agent_context_tokens)
        .or_else(|| known_context_size(model_name))
        .unwrap_or(global_default)
}

/// Extended resolver that also considers endpoint-based provider defaults.
/// Use this at agent creation / startup when the endpoint is known.
pub fn resolve_context_size_with_endpoint(
    agent_context_tokens: Option<u32>,
    model_name: &str,
    endpoint: Option<&str>,
    global_default: u32,
    probed_context: Option<u32>,
) -> u32 {
    probed_context
        .or(agent_context_tokens)
        .or_else(|| known_context_size(model_name))
        .or_else(|| endpoint.and_then(provider_default_context_size))
        .unwrap_or(global_default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_context_size_claude_models() {
        assert_eq!(
            known_context_size("claude-sonnet-4-20250514"),
            Some(200_000)
        );
        assert_eq!(known_context_size("claude-opus-4-20250514"), Some(200_000));
        assert_eq!(
            known_context_size("claude-3-5-haiku-20241022"),
            Some(200_000)
        );
    }

    #[test]
    fn known_context_size_openai_models() {
        assert_eq!(known_context_size("gpt-4o-2024-08-06"), Some(128_000));
        assert_eq!(known_context_size("o3-2025-04-16"), Some(200_000));
        assert_eq!(known_context_size("o1-mini"), Some(200_000));
        assert_eq!(known_context_size("gpt-4.1-2025-04-14"), Some(1_000_000));
    }

    #[test]
    fn known_context_size_grok_models() {
        assert_eq!(known_context_size("grok-4-0805"), Some(2_000_000));
        assert_eq!(known_context_size("grok-3-mini-beta"), Some(131_072));
    }

    #[test]
    fn known_context_size_returns_none_for_unknown() {
        assert_eq!(known_context_size("my-custom-model-v1"), None);
    }

    #[test]
    fn provider_default_anthropic() {
        assert_eq!(
            provider_default_context_size("https://api.anthropic.com/v1"),
            Some(200_000)
        );
    }

    #[test]
    fn provider_default_openai() {
        assert_eq!(
            provider_default_context_size("https://api.openai.com/v1"),
            Some(128_000)
        );
    }

    #[test]
    fn provider_default_google() {
        assert_eq!(
            provider_default_context_size("https://generativelanguage.googleapis.com/v1beta"),
            Some(1_000_000)
        );
    }

    #[test]
    fn provider_default_unknown_returns_none() {
        assert_eq!(
            provider_default_context_size("https://my-custom-llm.local/v1"),
            None
        );
    }

    #[test]
    fn resolve_with_endpoint_uses_provider_when_model_unknown() {
        let size = resolve_context_size_with_endpoint(
            None,
            "my-custom-model",
            Some("https://api.anthropic.com/v1"),
            125_000,
            None,
        );
        assert_eq!(size, 200_000);
    }

    #[test]
    fn resolve_with_endpoint_prefers_model_over_provider() {
        let size = resolve_context_size_with_endpoint(
            None,
            "gpt-4o",
            Some("https://api.openai.com/v1"),
            125_000,
            None,
        );
        assert_eq!(size, 128_000);
    }

    #[test]
    fn resolve_with_endpoint_falls_back_to_global_default() {
        let size = resolve_context_size_with_endpoint(
            None,
            "unknown-model",
            Some("https://unknown-provider.local"),
            125_000,
            None,
        );
        assert_eq!(size, 125_000);
    }
}
