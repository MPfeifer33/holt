use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::app_config::PluginEntry;

/// Scan a directory for plugin executables.
///
/// On Windows, looks for `.exe` files. On Unix, checks the executable permission bit.
pub fn scan_plugin_directory(dir: &Path) -> Vec<PathBuf> {
    if !dir.is_dir() {
        return Vec::new();
    }

    let mut found = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => {
            return Vec::new();
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && is_executable(&path) {
            found.push(path);
        }
    }

    found
}

/// Match plugin config entries to discovered binaries by filename stem.
///
/// Returns tuples of (plugin_id, config_entry, binary_path) for each match.
pub fn match_config_to_binaries(
    registry: &HashMap<String, PluginEntry>,
    found_binaries: &[PathBuf],
) -> Vec<(String, PluginEntry, PathBuf)> {
    let mut matched = Vec::new();

    for (id, entry) in registry {
        if !entry.enabled {
            continue;
        }

        // Find a binary whose stem matches the config's `binary` field
        let target_stem = &entry.binary;
        if let Some(binary_path) = found_binaries.iter().find(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s == target_stem.as_str())
                .unwrap_or(false)
        }) {
            matched.push((id.clone(), entry.clone(), binary_path.clone()));
        }
    }

    matched
}

/// Check if a file is executable.
#[cfg(target_os = "windows")]
fn is_executable(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("exe"))
        .unwrap_or(false)
}

#[cfg(not(target_os = "windows"))]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::app_config::PluginType;
    use tempfile::TempDir;

    #[test]
    fn test_scan_empty_directory() {
        let dir = TempDir::new().unwrap();
        let result = scan_plugin_directory(dir.path());
        assert!(result.is_empty());
    }

    #[test]
    fn test_scan_nonexistent_directory() {
        let result = scan_plugin_directory(Path::new("/nonexistent/path"));
        assert!(result.is_empty());
    }

    #[test]
    fn test_match_config_to_binaries_no_match() {
        let mut registry = HashMap::new();
        registry.insert(
            "test-plugin".to_string(),
            PluginEntry {
                enabled: true,
                plugin_type: PluginType::Native,
                binary: "nonexistent".to_string(),
                command: None,
                args: None,
                settings: HashMap::new(),
            },
        );

        let binaries = vec![];
        let matched = match_config_to_binaries(&registry, &binaries);
        assert!(matched.is_empty());
    }

    #[test]
    fn test_match_config_skips_disabled() {
        let mut registry = HashMap::new();
        registry.insert(
            "disabled-plugin".to_string(),
            PluginEntry {
                enabled: false,
                plugin_type: PluginType::Native,
                binary: "some-binary".to_string(),
                command: None,
                args: None,
                settings: HashMap::new(),
            },
        );

        let binaries = vec![PathBuf::from("/plugins/some-binary.exe")];
        let matched = match_config_to_binaries(&registry, &binaries);
        assert!(matched.is_empty());
    }

    #[test]
    fn test_match_config_finds_binary() {
        let mut registry = HashMap::new();
        registry.insert(
            "my-plugin".to_string(),
            PluginEntry {
                enabled: true,
                plugin_type: PluginType::Native,
                binary: "my-plugin".to_string(),
                command: None,
                args: None,
                settings: HashMap::new(),
            },
        );

        #[cfg(target_os = "windows")]
        let binaries = vec![PathBuf::from("C:/plugins/my-plugin.exe")];
        #[cfg(not(target_os = "windows"))]
        let binaries = vec![PathBuf::from("/plugins/my-plugin")];

        let matched = match_config_to_binaries(&registry, &binaries);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].0, "my-plugin");
    }
}
