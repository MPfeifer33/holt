/// Truncate tool result content per the configured strategy.
pub fn truncate_tool_result(
    content: &str,
    max_tokens: u32,
    char_ratio: f32,
    strategy: &str,
) -> String {
    let max_chars = (max_tokens as f32 * char_ratio) as usize;
    if content.len() <= max_chars {
        return content.to_string();
    }

    match strategy {
        "head_tail" => {
            let half = max_chars / 2;
            let head = safe_truncate(content, half);
            let tail = safe_truncate_tail(content, half);
            format!(
                "{}...[TRUNCATED {} chars]...{}",
                head,
                content.len() - max_chars,
                tail
            )
        }
        "head_only" => {
            let head = safe_truncate(content, max_chars.saturating_sub(30));
            format!("{}...[TRUNCATED]", head)
        }
        "tail_only" => {
            let tail = safe_truncate_tail(content, max_chars.saturating_sub(30));
            format!("[TRUNCATED]...{}", tail)
        }
        _ => {
            // Default to head_tail
            let half = max_chars / 2;
            let head = safe_truncate(content, half);
            let tail = safe_truncate_tail(content, half);
            format!(
                "{}...[TRUNCATED {} chars]...{}",
                head,
                content.len() - max_chars,
                tail
            )
        }
    }
}

/// UTF-8 safe truncation from the start of a string.
pub(crate) fn safe_truncate(s: &str, max_chars: usize) -> &str {
    if let Some((idx, _)) = s.char_indices().nth(max_chars) {
        &s[..idx]
    } else {
        s
    }
}

/// UTF-8 safe truncation from the end of a string.
pub(crate) fn safe_truncate_tail(s: &str, max_chars: usize) -> &str {
    let total_chars = s.chars().count();
    if total_chars <= max_chars {
        return s;
    }
    let skip = total_chars - max_chars;
    if let Some((idx, _)) = s.char_indices().nth(skip) {
        &s[idx..]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_tool_result_head_tail() {
        let content = "a".repeat(1000);
        let truncated = truncate_tool_result(&content, 100, 1.0, "head_tail");
        assert!(truncated.len() < content.len());
        assert!(truncated.contains("TRUNCATED"));
    }

    #[test]
    fn test_truncate_tool_result_under_limit() {
        let content = "short content";
        let truncated = truncate_tool_result(content, 100, 4.0, "head_tail");
        assert_eq!(truncated, content);
    }

    #[test]
    fn test_safe_truncate_utf8() {
        let text = "Hello 🌍 World 🎉 End";
        let head = safe_truncate(text, 8);
        assert_eq!(head, "Hello 🌍 ");
        let tail = safe_truncate_tail(text, 5);
        assert_eq!(tail, "🎉 End");
    }
}
