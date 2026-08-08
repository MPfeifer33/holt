use std::path::{Path, PathBuf};

use super::types::{ToolError, ToolErrorCode};

/// Canonicalize a requested path, resolving relative paths against `working_dir`.
/// Returns the canonical form without performing any boundary check.
fn canonicalize_requested(requested: &str, working_dir: &Path) -> Result<PathBuf, ToolError> {
    let requested_path = if Path::new(requested).is_absolute() {
        PathBuf::from(requested)
    } else {
        working_dir.join(requested)
    };

    if requested_path.exists() {
        dunce::canonicalize(&requested_path).map_err(|e| ToolError {
            code: ToolErrorCode::FileNotFound,
            message: format!("Failed to resolve path '{}': {}", requested, e),
            retryable: false,
        })
    } else {
        // Find the deepest existing ancestor and canonicalize that
        let mut ancestor = requested_path.clone();
        let mut trailing_parts = Vec::new();

        while !ancestor.exists() {
            if let Some(file_name) = ancestor.file_name() {
                trailing_parts.push(file_name.to_owned());
            } else {
                break;
            }
            if !ancestor.pop() {
                break;
            }
        }

        let mut canonical = if ancestor.exists() {
            dunce::canonicalize(&ancestor).map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Failed to resolve path '{}': {}", requested, e),
                retryable: false,
            })?
        } else {
            ancestor
        };

        for part in trailing_parts.into_iter().rev() {
            canonical.push(part);
        }
        Ok(canonical)
    }
}

/// Validate that a requested path is within the workspace root.
///
/// Resolves relative paths against `working_dir` (the agent's home base / CWD),
/// then checks that the resolved path falls within `workspace_root` (the outer
/// sandbox boundary). Uses `dunce` on Windows to strip `\\?\` prefixes.
///
/// This is the primary validation function. Agents can access any path within
/// the workspace root, not just their own working directory.
pub fn validate_path_with_root(
    requested: &str,
    working_dir: &Path,
    workspace_root: &Path,
) -> Result<PathBuf, ToolError> {
    let canonical_requested = canonicalize_requested(requested, working_dir)?;

    // Canonicalize workspace root (must exist)
    let canonical_root = dunce::canonicalize(workspace_root).map_err(|e| ToolError {
        code: ToolErrorCode::InternalError,
        message: format!("Failed to canonicalize workspace root: {}", e),
        retryable: false,
    })?;

    if !canonical_requested.starts_with(&canonical_root) {
        return Err(ToolError {
            code: ToolErrorCode::PathOutOfBounds,
            message: format!(
                "Path '{}' is outside the workspace root '{}'",
                requested,
                workspace_root.display()
            ),
            retryable: false,
        });
    }

    Ok(canonical_requested)
}

/// Validate that a requested path is within the agent's working directory.
///
/// Legacy wrapper — treats `working_dir` as both the CWD and the boundary.
/// Prefer `validate_path_with_root` for production use.
pub fn validate_path(requested: &str, working_dir: &Path) -> Result<PathBuf, ToolError> {
    validate_path_with_root(requested, working_dir, working_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // --- Legacy validate_path tests (backward compat) ---

    #[test]
    fn test_relative_path_within_boundary() {
        let tmp = TempDir::new().unwrap();
        let subdir = tmp.path().join("src");
        std::fs::create_dir(&subdir).unwrap();
        std::fs::write(subdir.join("main.rs"), "fn main() {}").unwrap();

        let result = validate_path("src/main.rs", tmp.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_path_escape_rejected() {
        let tmp = TempDir::new().unwrap();
        let result = validate_path("../../etc/passwd", tmp.path());
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.code, ToolErrorCode::PathOutOfBounds);
        }
    }

    #[test]
    fn test_nonexistent_file_within_boundary() {
        let tmp = TempDir::new().unwrap();
        let result = validate_path("new_file.txt", tmp.path());
        assert!(result.is_ok());
    }

    // --- validate_path_with_root tests ---

    #[test]
    fn test_path_within_workspace_root_but_outside_working_dir() {
        // Key behavioral change: agents can access sibling dirs within workspace root
        let tmp = TempDir::new().unwrap();
        let frontend = tmp.path().join("frontend");
        let backend = tmp.path().join("backend");
        std::fs::create_dir(&frontend).unwrap();
        std::fs::create_dir(&backend).unwrap();
        std::fs::write(backend.join("server.rs"), "fn main() {}").unwrap();

        // Agent's working_dir is frontend, but workspace root is tmp
        let result = validate_path_with_root(
            &backend.join("server.rs").to_string_lossy(),
            &frontend,
            tmp.path(),
        );
        assert!(
            result.is_ok(),
            "Agent should access sibling dir within workspace root"
        );
    }

    #[test]
    fn test_path_outside_workspace_root_rejected() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();

        let result = validate_path_with_root(
            &outside.join("secret.txt").to_string_lossy(),
            &workspace,
            &workspace,
        );
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.code, ToolErrorCode::PathOutOfBounds);
        }
    }

    #[test]
    fn test_relative_path_resolves_against_working_dir() {
        let tmp = TempDir::new().unwrap();
        let subdir = tmp.path().join("project").join("src");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(subdir.join("lib.rs"), "// lib").unwrap();

        // working_dir = project/src, workspace_root = tmp
        let result = validate_path_with_root("lib.rs", &subdir, tmp.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_dotdot_traversal_within_workspace_root_allowed() {
        let tmp = TempDir::new().unwrap();
        let sub_a = tmp.path().join("a");
        let sub_b = tmp.path().join("b");
        std::fs::create_dir(&sub_a).unwrap();
        std::fs::create_dir(&sub_b).unwrap();
        std::fs::write(sub_b.join("data.txt"), "data").unwrap();

        // Agent in /a, traverses ../b/data.txt — still within workspace root
        let result = validate_path_with_root("../b/data.txt", &sub_a, tmp.path());
        assert!(
            result.is_ok(),
            "../ traversal within workspace root should be allowed"
        );
    }

    #[test]
    fn test_dotdot_traversal_escaping_workspace_root_rejected() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let agent_dir = workspace.join("agent");
        std::fs::create_dir_all(&agent_dir).unwrap();

        // Agent in workspace/agent, traverses ../../ — escapes workspace root
        let result = validate_path_with_root("../../etc/passwd", &agent_dir, &workspace);
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.code, ToolErrorCode::PathOutOfBounds);
        }
    }

    // --- Symlink tests (verify dunce::canonicalize resolves symlinks) ---

    #[cfg(unix)]
    #[test]
    fn test_symlink_within_boundary_allowed() {
        let tmp = TempDir::new().unwrap();
        let real_file = tmp.path().join("real.txt");
        std::fs::write(&real_file, "content").unwrap();

        let link = tmp.path().join("link.txt");
        std::os::unix::fs::symlink(&real_file, &link).unwrap();

        let result = validate_path(&link.to_string_lossy(), tmp.path());
        assert!(
            result.is_ok(),
            "symlink to file within boundary should be allowed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_escaping_boundary_rejected() {
        let outer = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();

        let secret = outer.path().join("secret.txt");
        std::fs::write(&secret, "secret").unwrap();

        let link = workspace.path().join("escape.txt");
        std::os::unix::fs::symlink(&secret, &link).unwrap();

        let result = validate_path(&link.to_string_lossy(), workspace.path());
        assert!(
            result.is_err(),
            "symlink pointing outside boundary should be rejected"
        );
        if let Err(e) = result {
            assert_eq!(e.code, ToolErrorCode::PathOutOfBounds);
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_chain_escaping_rejected() {
        let outer = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();

        let secret = outer.path().join("secret.txt");
        std::fs::write(&secret, "secret").unwrap();

        // Create chain: link1 → link2 → secret (outside boundary)
        let link2 = outer.path().join("link2.txt");
        std::os::unix::fs::symlink(&secret, &link2).unwrap();

        let link1 = workspace.path().join("link1.txt");
        std::os::unix::fs::symlink(&link2, &link1).unwrap();

        let result = validate_path(&link1.to_string_lossy(), workspace.path());
        assert!(
            result.is_err(),
            "symlink chain escaping boundary should be rejected"
        );
        if let Err(e) = result {
            assert_eq!(e.code, ToolErrorCode::PathOutOfBounds);
        }
    }
}
