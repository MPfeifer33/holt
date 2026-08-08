//! Cache sentinel markers for cross-provider prompt caching abstraction.
//!
//! Sentinel markers are Private Use Area (PUA) Unicode characters inserted at
//! cache boundaries in the assembled system prompt. Provider adapters convert
//! these markers into provider-specific cache controls before sending to the API.
//!
//! This decouples cache strategy (defined in the prompt assembler) from
//! provider implementation (handled by adapters).

/// 1-hour TTL boundary. Content before this marker is stable across the session
/// (tools, system prompt, persona). Providers that support explicit TTLs cache
/// this region for up to 1 hour.
pub const CACHE_SENTINEL_1HR: &str = "\u{E001}";

/// 5-minute TTL boundary. Content before this marker is semi-stable (hot memories,
/// conversation prefix). Uses default provider caching behavior.
pub const CACHE_SENTINEL_5M: &str = "\u{E002}";

/// Explicitly non-cacheable region marker. Content before this marker should NOT
/// be cached (dynamic working memory, per-turn retrieval). Providers strip this
/// marker without applying any cache control.
pub const CACHE_SENTINEL_NOCACHE: &str = "\u{E003}";

/// All sentinel characters for stripping. Provider adapters that don't use sentinels
/// (or need to strip them from final output) can use this slice.
pub const ALL_SENTINELS: &[&str] = &[
    CACHE_SENTINEL_1HR,
    CACHE_SENTINEL_5M,
    CACHE_SENTINEL_NOCACHE,
];

/// Strip all sentinel markers from a string. Used by provider adapters that handle
/// caching through other mechanisms (OpenAI auto-prefix, local models).
pub fn strip_sentinels(input: &str) -> String {
    let mut result = input.to_string();
    for sentinel in ALL_SENTINELS {
        result = result.replace(sentinel, "");
    }
    result
}

/// Split content at sentinel boundaries. Returns a list of (content, sentinel_type) pairs.
/// The last segment has no sentinel (it's the trailing content after the final marker).
pub fn split_at_sentinels(input: &str) -> Vec<ContentRegion> {
    let mut regions = Vec::new();
    let mut remaining = input;

    loop {
        // Find the earliest sentinel in the remaining string
        let earliest = ALL_SENTINELS
            .iter()
            .filter_map(|s| remaining.find(s).map(|pos| (pos, *s)))
            .min_by_key(|(pos, _)| *pos);

        match earliest {
            Some((pos, sentinel)) => {
                let content = &remaining[..pos];
                if !content.is_empty() {
                    let cache_hint = match sentinel {
                        s if s == CACHE_SENTINEL_1HR => CacheHint::LongTtl,
                        s if s == CACHE_SENTINEL_5M => CacheHint::ShortTtl,
                        s if s == CACHE_SENTINEL_NOCACHE => CacheHint::NoCache,
                        _ => CacheHint::NoCache,
                    };
                    regions.push(ContentRegion {
                        content: content.to_string(),
                        cache_hint,
                    });
                }
                remaining = &remaining[pos + sentinel.len()..];
            }
            None => {
                // No more sentinels — remaining is the final region
                if !remaining.is_empty() {
                    regions.push(ContentRegion {
                        content: remaining.to_string(),
                        cache_hint: CacheHint::None,
                    });
                }
                break;
            }
        }
    }

    regions
}

/// A segment of prompt content with its associated cache hint.
#[derive(Debug, Clone, PartialEq)]
pub struct ContentRegion {
    pub content: String,
    pub cache_hint: CacheHint,
}

/// Cache behavior hint derived from sentinel markers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CacheHint {
    /// Cache with long TTL (1 hour). For deployment-stable content.
    LongTtl,
    /// Cache with short TTL (5 minutes). For semi-stable content.
    ShortTtl,
    /// Do not cache. For dynamic per-turn content.
    NoCache,
    /// No sentinel was found after this region. Trailing content.
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_sentinels_removes_all() {
        let input = format!(
            "stable content{}semi-stable{}dynamic{}trailing",
            CACHE_SENTINEL_1HR, CACHE_SENTINEL_5M, CACHE_SENTINEL_NOCACHE
        );
        let stripped = strip_sentinels(&input);
        assert_eq!(stripped, "stable contentsemi-stabledynamictrailing");
        assert!(!stripped.contains('\u{E001}'));
        assert!(!stripped.contains('\u{E002}'));
        assert!(!stripped.contains('\u{E003}'));
    }

    #[test]
    fn test_strip_sentinels_no_markers() {
        let input = "plain text with no markers";
        assert_eq!(strip_sentinels(input), input);
    }

    #[test]
    fn test_split_at_sentinels_basic() {
        let input = format!(
            "tools and system{}persona{}memories{}conversation",
            CACHE_SENTINEL_1HR, CACHE_SENTINEL_5M, CACHE_SENTINEL_NOCACHE
        );
        let regions = split_at_sentinels(&input);
        assert_eq!(regions.len(), 4);

        assert_eq!(regions[0].content, "tools and system");
        assert_eq!(regions[0].cache_hint, CacheHint::LongTtl);

        assert_eq!(regions[1].content, "persona");
        assert_eq!(regions[1].cache_hint, CacheHint::ShortTtl);

        assert_eq!(regions[2].content, "memories");
        assert_eq!(regions[2].cache_hint, CacheHint::NoCache);

        assert_eq!(regions[3].content, "conversation");
        assert_eq!(regions[3].cache_hint, CacheHint::None);
    }

    #[test]
    fn test_split_at_sentinels_no_markers() {
        let input = "just plain text";
        let regions = split_at_sentinels(input);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].content, "just plain text");
        assert_eq!(regions[0].cache_hint, CacheHint::None);
    }

    #[test]
    fn test_split_at_sentinels_empty_regions_skipped() {
        // Adjacent sentinels with no content between them
        let input = format!("content{}{}trailing", CACHE_SENTINEL_1HR, CACHE_SENTINEL_5M);
        let regions = split_at_sentinels(&input);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].content, "content");
        assert_eq!(regions[0].cache_hint, CacheHint::LongTtl);
        assert_eq!(regions[1].content, "trailing");
        assert_eq!(regions[1].cache_hint, CacheHint::None);
    }

    #[test]
    fn test_sentinels_are_invisible_pua_chars() {
        // Verify these are actually in the Private Use Area
        assert_eq!(CACHE_SENTINEL_1HR.chars().next().unwrap() as u32, 0xE001);
        assert_eq!(CACHE_SENTINEL_5M.chars().next().unwrap() as u32, 0xE002);
        assert_eq!(
            CACHE_SENTINEL_NOCACHE.chars().next().unwrap() as u32,
            0xE003
        );
    }
}
