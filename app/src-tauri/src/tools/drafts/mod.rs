pub mod delete;
pub mod list;
pub mod promote;
pub mod read;
pub mod write;

use std::path::PathBuf;

/// Get the drafts directory for an agent.
pub fn drafts_dir(agent_id: &str) -> PathBuf {
    crate::config::app_config::agent_dir(agent_id).join("drafts")
}

/// Sanitize a draft name to prevent path traversal.
/// Allows alphanumeric, hyphens, underscores, and interior dots (for extensions).
/// Strips leading/trailing dots and consecutive dots (no `..` traversal).
/// Appends .md if no extension is present.
pub fn sanitize_name(name: &str) -> String {
    // Keep safe chars including dots
    let sanitized: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();

    // Collapse consecutive dots to prevent ".." traversal patterns
    let mut collapsed = String::with_capacity(sanitized.len());
    let mut last_was_dot = false;
    for c in sanitized.chars() {
        if c == '.' {
            if !last_was_dot {
                collapsed.push(c);
            }
            last_was_dot = true;
        } else {
            collapsed.push(c);
            last_was_dot = false;
        }
    }

    // Strip leading/trailing dots
    let trimmed = collapsed.trim_matches('.');

    if trimmed.is_empty() {
        return "untitled.md".to_string();
    }

    // If no extension (no dot in the name), append .md
    if !trimmed.contains('.') {
        format!("{}.md", trimmed)
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_normal_name() {
        assert_eq!(sanitize_name("my-draft"), "my-draft.md");
    }

    #[test]
    fn test_sanitize_preserves_extension() {
        // Key fix: dots in names are preserved, not stripped
        assert_eq!(sanitize_name("notes.md"), "notes.md");
    }

    #[test]
    fn test_sanitize_preserves_non_md_extension() {
        assert_eq!(sanitize_name("config.toml"), "config.toml");
    }

    #[test]
    fn test_sanitize_path_traversal_blocked() {
        // Path separators stripped, consecutive dots collapsed
        let result = sanitize_name("../../etc/passwd");
        assert!(!result.contains(".."));
        assert!(!result.contains('/'));
    }

    #[test]
    fn test_sanitize_preserves_version_dots() {
        assert_eq!(sanitize_name("my.draft.v2"), "my.draft.v2");
    }

    #[test]
    fn test_sanitize_empty() {
        assert_eq!(sanitize_name(""), "untitled.md");
    }

    #[test]
    fn test_sanitize_special_chars() {
        assert_eq!(sanitize_name("hello world!@#"), "helloworld.md");
    }

    #[test]
    fn test_sanitize_leading_dots_stripped() {
        assert_eq!(sanitize_name(".hidden"), "hidden.md");
    }

    #[test]
    fn test_sanitize_trailing_dots_stripped() {
        assert_eq!(sanitize_name("file."), "file.md");
    }

    #[test]
    fn test_sanitize_consecutive_dots_collapsed() {
        let result = sanitize_name("a..b...c");
        assert_eq!(result, "a.b.c");
    }

    #[test]
    fn test_sanitize_only_dots() {
        assert_eq!(sanitize_name("..."), "untitled.md");
    }
}
