/// The Claude model an agent gets when nothing names one. The one place
/// this is written down; bump it when the Opus line moves.
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-opus-5-5";

/// Short names people type for the current Claude line-up, and the older
/// models still worth reaching by name. `None` for anything else.
pub fn claude_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "opus" | "opus5.5" => "claude-opus-5-5",
        "opus5" => "claude-opus-5",
        "opus4.8" => "claude-opus-4-8",
        "opus4.6" => "claude-opus-4-6",
        "sonnet" | "sonnet5.5" => "claude-sonnet-5-5",
        "sonnet5" => "claude-sonnet-5",
        "sonnet4.6" => "claude-sonnet-4-6",
        "haiku" | "haiku5.5" => "claude-haiku-5-5",
        "haiku4.5" => "claude-haiku-4-5",
        "fable" | "fable5.1" => "claude-fable-5-1",
        "fable5" => "claude-fable-5",
        _ => return None,
    })
}

/// Lookup table of known model context sizes (Tier 2 detection).
///
/// Matches on model name substrings. More specific matches are checked first
/// to avoid false positives (e.g. "grok-4" before generic "grok").
/// Sizes are from the providers' model pages as of November 2026.
pub fn known_context_size(model_name: &str) -> Option<u32> {
    let name = model_name.to_lowercase();

    // --- Anthropic ---
    if name.contains("claude") {
        // 4.5 and earlier (the manual-thinking-budget generation) are 200k;
        // everything from 4.6 on is 1M. An unrecognised id is assumed current.
        if crate::providers::anthropic::takes_thinking_budget(&name) {
            return Some(200_000);
        }
        return Some(1_000_000);
    }

    // --- Google ---
    if name.contains("gemini") {
        return Some(1_000_000);
    }

    // --- OpenAI ---
    if name.contains("o1") || name.contains("o3") || name.contains("o4") {
        return Some(200_000);
    }
    if name.contains("gpt-4.1") || name.contains("gpt-4-1") {
        return Some(1_000_000);
    }
    if name.contains("gpt-4") || name.contains("gpt-3") {
        return Some(128_000);
    }
    if name.contains("gpt-5.") || name.contains("gpt-6") {
        return Some(1_050_000);
    }
    if name.contains("gpt-") {
        // Bare gpt-5 and anything unrecognised: the GPT-5 floor.
        return Some(400_000);
    }

    // --- xAI ---
    if name.contains("grok") {
        return Some(grok_context_size(&name));
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

/// Context window for a Grok model id. xAI publishes context sizes but no
/// output caps.
pub fn grok_context_size(model: &str) -> u32 {
    let name = model.to_lowercase();
    if name.contains("grok-4.3") {
        1_000_000
    } else if name.contains("grok-4.") {
        // 4.5, 4.6, 4.7 and whatever follows them.
        500_000
    } else if name.contains("grok-4") {
        256_000
    } else {
        131_072
    }
}

/// Endpoint-based fallback when model name isn't recognized.
/// Returns a conservative default for the provider so we don't undersize
/// the context budget for unknown/custom model names.
pub fn provider_default_context_size(endpoint: &str) -> Option<u32> {
    let ep = endpoint.to_lowercase();
    if ep.contains("anthropic") {
        return Some(1_000_000);
    }
    if ep.contains("generativelanguage.googleapis.com") {
        return Some(1_000_000);
    }
    if ep.contains("api.openai.com") {
        return Some(400_000);
    }
    if ep.contains("api.x.ai") {
        return Some(256_000);
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
        assert_eq!(known_context_size("claude-haiku-4-5"), Some(200_000));
        for current in [
            "claude-opus-5-5",
            "claude-opus-4-6",
            "claude-sonnet-5",
            "claude-fable-5-1",
        ] {
            assert_eq!(known_context_size(current), Some(1_000_000), "{current}");
        }
    }

    #[test]
    fn every_alias_and_the_default_name_a_current_model() {
        for alias in ["opus", "sonnet", "haiku", "fable", "opus4.6", "haiku4.5"] {
            let id = claude_alias(alias).expect(alias);
            assert!(id.starts_with("claude-"), "{alias} -> {id}");
            assert!(known_context_size(id).is_some());
        }
        assert_eq!(claude_alias("opus"), Some(DEFAULT_CLAUDE_MODEL));
        assert_eq!(claude_alias("sonnet4.5"), None, "retired aliases are gone");
        assert_eq!(claude_alias("gpt-5.5"), None);
    }

    #[test]
    fn known_context_size_openai_models() {
        assert_eq!(known_context_size("gpt-4o-2024-08-06"), Some(128_000));
        assert_eq!(known_context_size("o3-2025-04-16"), Some(200_000));
        assert_eq!(known_context_size("o1-mini"), Some(200_000));
        assert_eq!(known_context_size("gpt-4.1-2025-04-14"), Some(1_000_000));
        assert_eq!(known_context_size("gpt-5.5"), Some(1_050_000));
        assert_eq!(known_context_size("gpt-6-astra"), Some(1_050_000));
        assert_eq!(known_context_size("gpt-5"), Some(400_000));
    }

    #[test]
    fn known_context_size_grok_models() {
        assert_eq!(known_context_size("grok-4-0805"), Some(256_000));
        assert_eq!(known_context_size("grok-4.3"), Some(1_000_000));
        assert_eq!(known_context_size("grok-4.7"), Some(500_000));
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
            Some(1_000_000)
        );
    }

    #[test]
    fn provider_default_openai() {
        assert_eq!(
            provider_default_context_size("https://api.openai.com/v1"),
            Some(400_000)
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
        assert_eq!(size, 1_000_000);
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
