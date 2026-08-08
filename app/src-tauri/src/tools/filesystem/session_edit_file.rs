//! Path-restricted edit_file for session memory extraction forks.
//!
//! Wraps the standard EditFileTool but rejects any path that doesn't match
//! the allowed session memory file. This prevents extraction forks from
//! modifying arbitrary files.

use std::path::PathBuf;

use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

/// A path-restricted variant of `edit_file` that only allows editing
/// a single pre-specified file (the session memory file).
pub struct SessionEditFileTool {
    allowed_path: PathBuf,
    inner: super::edit_file::EditFileTool,
}

impl SessionEditFileTool {
    pub fn new(allowed_path: PathBuf) -> Self {
        Self {
            allowed_path,
            inner: super::edit_file::EditFileTool,
        }
    }

    /// Check whether a given path string resolves to the allowed path.
    /// Rejects traversal attempts by checking for `..` sequences.
    pub fn is_path_allowed(&self, path: &str) -> bool {
        if path.contains("..") {
            return false;
        }
        std::path::Path::new(path) == self.allowed_path
    }
}

#[async_trait::async_trait]
impl Tool for SessionEditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Apply targeted search-and-replace edits to the session memory file. Each edit's old_text must match exactly once. Atomic: all edits succeed or none apply."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.inner.parameters_schema()
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let path_str = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: path".to_string(),
                retryable: false,
            })?;

        if !self.is_path_allowed(path_str) {
            return Err(ToolError {
                code: ToolErrorCode::PermissionDenied,
                message: "Session memory extractor can only edit the session file.".to_string(),
                retryable: false,
            });
        }

        self.inner.execute(arguments, context).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_not_matching_allowed() {
        let tool = SessionEditFileTool::new(PathBuf::from("/allowed/session.md"));
        assert!(!tool.is_path_allowed("/other/file.rs"));
        assert!(tool.is_path_allowed("/allowed/session.md"));
    }

    #[test]
    fn rejects_traversal_attempt() {
        let tool = SessionEditFileTool::new(PathBuf::from("/data/sessions/sess.md"));
        assert!(!tool.is_path_allowed("/data/sessions/../secrets.md"));
        assert!(!tool.is_path_allowed("../../../etc/passwd"));
    }

    #[test]
    fn name_is_edit_file() {
        let tool = SessionEditFileTool::new(PathBuf::from("/tmp/test.md"));
        assert_eq!(tool.name(), "edit_file");
    }
}
