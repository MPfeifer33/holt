use std::path::PathBuf;

/// Cross-platform shell configuration.
///
/// Detects the host OS and provides the appropriate shell binary and arguments
/// so agents work transparently on Windows, Linux, and macOS.
#[derive(Debug, Clone)]
pub struct ShellConfig {
    pub shell_binary: PathBuf,
    pub shell_args: Vec<String>,
    pub path_separator: char,
    pub line_ending: &'static str,
}

impl ShellConfig {
    /// Build config for the current OS.
    pub fn for_current_os() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self {
                shell_binary: PathBuf::from("powershell.exe"),
                shell_args: vec!["-NoProfile".to_string(), "-Command".to_string()],
                path_separator: '\\',
                line_ending: "\r\n",
            }
        }

        #[cfg(target_os = "macos")]
        {
            Self {
                shell_binary: PathBuf::from("/bin/zsh"),
                shell_args: vec!["-c".to_string()],
                path_separator: '/',
                line_ending: "\n",
            }
        }

        #[cfg(target_os = "linux")]
        {
            Self {
                shell_binary: PathBuf::from("/bin/bash"),
                shell_args: vec!["-c".to_string()],
                path_separator: '/',
                line_ending: "\n",
            }
        }
    }

    /// Get the OS name string for agent system prompts.
    pub fn os_name() -> &'static str {
        #[cfg(target_os = "windows")]
        {
            "windows"
        }
        #[cfg(target_os = "macos")]
        {
            "macos"
        }
        #[cfg(target_os = "linux")]
        {
            "linux"
        }
    }
}

/// PTY-specific shell configuration.
pub struct PtyConfig {
    pub shell_binary: PathBuf,
    pub shell_args: Vec<String>,
    pub env_vars: Vec<(String, String)>,
}

impl PtyConfig {
    pub fn for_current_os() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self {
                shell_binary: PathBuf::from("/bin/bash"),
                shell_args: vec!["--norc".to_string(), "--noprofile".to_string()],
                env_vars: vec![
                    ("TERM".to_string(), "dumb".to_string()),
                    ("NO_COLOR".to_string(), "1".to_string()),
                    ("LANG".to_string(), "C.UTF-8".to_string()),
                    ("HISTFILE".to_string(), "/dev/null".to_string()),
                ],
            }
        }
        #[cfg(target_os = "macos")]
        {
            Self {
                shell_binary: PathBuf::from("/bin/bash"),
                shell_args: vec!["--norc".to_string(), "--noprofile".to_string()],
                env_vars: vec![
                    ("TERM".to_string(), "dumb".to_string()),
                    ("NO_COLOR".to_string(), "1".to_string()),
                    ("LANG".to_string(), "C.UTF-8".to_string()),
                    ("HISTFILE".to_string(), "/dev/null".to_string()),
                ],
            }
        }
        #[cfg(target_os = "windows")]
        {
            Self {
                shell_binary: PathBuf::from("bash.exe"),
                shell_args: vec!["--norc".to_string(), "--noprofile".to_string()],
                env_vars: vec![
                    ("TERM".to_string(), "dumb".to_string()),
                    ("NO_COLOR".to_string(), "1".to_string()),
                ],
            }
        }
    }
}
