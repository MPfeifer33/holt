use serde::{Deserialize, Serialize};
use std::path::Path;

// ---------------------------------------------------------------------------
// URL Parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedSkillUrl {
    pub owner: String,
    pub repo: String,
    pub branch: String,
    pub path: String, // "." for root
}

pub fn parse_skill_url(url: &str) -> Result<ParsedSkillUrl, String> {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");

    // skills.sh URL: https://skills.sh/{owner}/{repo}/{skill}
    if let Some(rest) = url.strip_prefix("https://skills.sh/") {
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        if parts.len() < 3 {
            return Err("Invalid skills.sh URL: need owner/repo/skill".to_string());
        }
        return Ok(ParsedSkillUrl {
            owner: parts[0].to_string(),
            repo: parts[1].to_string(),
            branch: "main".to_string(),
            path: format!("skills/{}", parts[2]),
        });
    }

    // GitHub URLs: https://github.com/{owner}/{repo}[/tree|blob/{branch}/{path}]
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        let parts: Vec<&str> = rest.splitn(4, '/').collect();
        if parts.len() < 2 {
            return Err("Invalid GitHub URL: need at least owner/repo".to_string());
        }
        let owner = parts[0].to_string();
        let repo = parts[1].to_string();

        if parts.len() >= 4 && (parts[2] == "tree" || parts[2] == "blob") {
            let kind = parts[2];
            let remainder = &rest[owner.len() + 1 + repo.len() + 1 + kind.len() + 1..];
            let branch_end = remainder.find('/').unwrap_or(remainder.len());
            let branch = remainder[..branch_end].to_string();
            let full_path = if branch_end < remainder.len() {
                remainder[branch_end + 1..].to_string()
            } else {
                ".".to_string()
            };

            let path = if kind == "blob" && full_path != "." {
                // Strip filename to get directory
                match full_path.rfind('/') {
                    Some(pos) => full_path[..pos].to_string(),
                    None => ".".to_string(),
                }
            } else {
                full_path
            };

            return Ok(ParsedSkillUrl {
                owner,
                repo,
                branch,
                path,
            });
        }

        // Bare repo URL
        return Ok(ParsedSkillUrl {
            owner,
            repo,
            branch: "main".to_string(),
            path: ".".to_string(),
        });
    }

    Err(format!(
        "Unsupported URL format. Use a GitHub or skills.sh URL: {}",
        url
    ))
}

/// Check if a tree entry path belongs to the skill directory.
/// Uses exact prefix match with `/` boundary to avoid
/// `skills/frontend` matching `skills/frontend-design/`.
pub fn path_matches_skill_dir(entry_path: &str, skill_path: &str) -> bool {
    if skill_path == "." {
        return !entry_path.contains('/');
    }
    entry_path == skill_path || entry_path.starts_with(&format!("{}/", skill_path))
}

// ---------------------------------------------------------------------------
// Import Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportFile {
    pub name: String,
    pub content: String,
    pub size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPreview {
    pub skill_name: String,
    pub description: String,
    pub is_directory: bool,
    pub files: Vec<ImportFile>,
    pub source_url: String,
    pub commit_sha: String,
    pub already_exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SourceProvenance {
    url: String,
    commit_sha: String,
    installed_at: String,
    file_count: usize,
}

// ---------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------

pub fn install_skill_files(
    skills_dir: &Path,
    skill_name: &str,
    files: &[ImportFile],
    overwrite: bool,
    source_url: &str,
    commit_sha: &str,
) -> Result<(), String> {
    let is_multi = files.len() > 1 || files.iter().any(|f| f.name == "SKILL.md");

    if is_multi {
        let skill_dir = skills_dir.join(skill_name);
        if skill_dir.exists() && !overwrite {
            return Err(format!("Skill '{}' already exists", skill_name));
        }
        if overwrite && skill_dir.exists() {
            std::fs::remove_dir_all(&skill_dir)
                .map_err(|e| format!("Failed to remove existing skill: {}", e))?;
        }
        std::fs::create_dir_all(&skill_dir)
            .map_err(|e| format!("Failed to create skill directory: {}", e))?;

        for file in files {
            std::fs::write(skill_dir.join(&file.name), &file.content)
                .map_err(|e| format!("Failed to write {}: {}", file.name, e))?;
        }

        // Write provenance inside directory
        let provenance = SourceProvenance {
            url: source_url.to_string(),
            commit_sha: commit_sha.to_string(),
            installed_at: chrono::Utc::now().to_rfc3339(),
            file_count: files.len(),
        };
        let provenance_json = serde_json::to_string_pretty(&provenance)
            .map_err(|e| format!("Failed to serialize provenance: {}", e))?;
        std::fs::write(skill_dir.join(".source.json"), provenance_json)
            .map_err(|e| format!("Failed to write provenance: {}", e))?;
    } else {
        let file = &files[0];
        let file_path = skills_dir.join(format!("{}.md", skill_name));
        if file_path.exists() && !overwrite {
            return Err(format!("Skill '{}' already exists", skill_name));
        }
        std::fs::write(&file_path, &file.content)
            .map_err(|e| format!("Failed to write skill file: {}", e))?;

        // Write provenance as dot-prefixed file
        let provenance = SourceProvenance {
            url: source_url.to_string(),
            commit_sha: commit_sha.to_string(),
            installed_at: chrono::Utc::now().to_rfc3339(),
            file_count: 1,
        };
        let provenance_json = serde_json::to_string_pretty(&provenance)
            .map_err(|e| format!("Failed to serialize provenance: {}", e))?;
        std::fs::write(
            skills_dir.join(format!(".source-{}.json", skill_name)),
            provenance_json,
        )
        .map_err(|e| format!("Failed to write provenance: {}", e))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// GitHub Fetch
// ---------------------------------------------------------------------------

/// Fetch skill files from GitHub for preview.
pub async fn fetch_from_github(
    parsed: &ParsedSkillUrl,
    source_url: &str,
) -> Result<ImportPreview, String> {
    let client = reqwest::Client::builder()
        .user_agent("Holt-Skill-Import")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    // 1. Fetch tree
    let tree_url = format!(
        "https://api.github.com/repos/{}/{}/git/trees/{}?recursive=1",
        parsed.owner, parsed.repo, parsed.branch
    );
    let tree_resp = client
        .get(&tree_url)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch GitHub tree: {}", e))?;

    if tree_resp.status().as_u16() == 404 && parsed.branch == "main" {
        // Retry with master
        let mut retry_parsed = parsed.clone();
        retry_parsed.branch = "master".to_string();
        return Box::pin(fetch_from_github(&retry_parsed, source_url)).await;
    }

    if tree_resp.status().as_u16() == 403 {
        return Err("GitHub API rate limit reached. Try again in a few minutes.".to_string());
    }

    if !tree_resp.status().is_success() {
        return Err(format!(
            "Repository or path not found. Check the URL. (HTTP {})",
            tree_resp.status()
        ));
    }

    let tree_json: serde_json::Value = tree_resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse GitHub tree: {}", e))?;

    // 2. Filter for .md files in skill path
    let tree_entries = tree_json["tree"]
        .as_array()
        .ok_or("Invalid tree response from GitHub")?;

    let md_entries: Vec<&serde_json::Value> = tree_entries
        .iter()
        .filter(|e| {
            let path = e["path"].as_str().unwrap_or("");
            let entry_type = e["type"].as_str().unwrap_or("");
            entry_type == "blob"
                && path.ends_with(".md")
                && path_matches_skill_dir(path, &parsed.path)
        })
        .collect();

    if md_entries.is_empty() {
        return Err("No skill found at this path. Check the URL and try again.".to_string());
    }

    // 3. Fetch content for each file
    let mut files: Vec<ImportFile> = Vec::new();
    for entry in &md_entries {
        let full_path = entry["path"].as_str().unwrap_or("");
        let file_name = if parsed.path == "." {
            full_path.to_string()
        } else {
            full_path
                .strip_prefix(&format!("{}/", parsed.path))
                .unwrap_or(full_path.rsplit('/').next().unwrap_or(full_path))
                .to_string()
        };

        let raw_url = format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{}",
            parsed.owner, parsed.repo, parsed.branch, full_path
        );
        let content = client
            .get(&raw_url)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch {}: {}", file_name, e))?
            .text()
            .await
            .map_err(|e| format!("Failed to read {}: {}", file_name, e))?;

        files.push(ImportFile {
            size_bytes: content.len(),
            name: file_name,
            content,
        });
    }

    // 4. Fetch commit SHA
    let commit_url = format!(
        "https://api.github.com/repos/{}/{}/commits/{}",
        parsed.owner, parsed.repo, parsed.branch
    );
    let commit_sha = match client.get(&commit_url).send().await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(j) => j["sha"]
                .as_str()
                .map(|s| s.chars().take(7).collect::<String>())
                .unwrap_or_else(|| "unknown".to_string()),
            Err(_) => "unknown".to_string(),
        },
        Err(_) => "unknown".to_string(),
    };

    // 5. Parse skill name from SKILL.md or first file
    let primary_file = files
        .iter()
        .find(|f| f.name == "SKILL.md")
        .unwrap_or(&files[0]);
    let skill_name = extract_frontmatter_field(&primary_file.content, "name")
        .unwrap_or_else(|| parsed.repo.clone());
    let description =
        extract_frontmatter_field(&primary_file.content, "description").unwrap_or_default();

    let is_directory = files.len() > 1 || files.iter().any(|f| f.name == "SKILL.md");

    Ok(ImportPreview {
        skill_name,
        description,
        is_directory,
        files,
        source_url: source_url.to_string(),
        commit_sha,
        already_exists: false, // Set by Tauri command wrapper
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn extract_frontmatter_field(content: &str, field: &str) -> Option<String> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_open = &trimmed[3..];
    let close_pos = after_open.find("\n---")?;
    let yaml_block = &after_open[..close_pos];
    for line in yaml_block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(&format!("{}:", field)) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Install tests --

    #[test]
    fn test_install_single_file_skill() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![ImportFile {
            name: "my-skill.md".to_string(),
            content: "---\nname: my-skill\ndescription: Test\n---\nBody.".to_string(),
            size_bytes: 50,
        }];

        install_skill_files(
            dir.path(),
            "my-skill",
            &files,
            false,
            "https://example.com",
            "abc123",
        )
        .unwrap();

        assert!(dir.path().join("my-skill.md").exists());
        assert!(dir.path().join(".source-my-skill.json").exists());
    }

    #[test]
    fn test_install_multi_file_skill() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            ImportFile {
                name: "SKILL.md".to_string(),
                content: "---\nname: tdd\ndescription: TDD\n---\nMain.".to_string(),
                size_bytes: 40,
            },
            ImportFile {
                name: "tests.md".to_string(),
                content: "Test content.".to_string(),
                size_bytes: 13,
            },
        ];

        install_skill_files(
            dir.path(),
            "tdd",
            &files,
            false,
            "https://example.com",
            "def456",
        )
        .unwrap();

        assert!(dir.path().join("tdd").join("SKILL.md").exists());
        assert!(dir.path().join("tdd").join("tests.md").exists());
        assert!(dir.path().join("tdd").join(".source.json").exists());
    }

    #[test]
    fn test_install_conflict_no_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("existing.md"),
            "---\nname: existing\ndescription: Old\n---\nOld.",
        )
        .unwrap();

        let files = vec![ImportFile {
            name: "existing.md".to_string(),
            content: "New content.".to_string(),
            size_bytes: 12,
        }];

        let result = install_skill_files(dir.path(), "existing", &files, false, "", "");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[test]
    fn test_install_overwrite_replaces() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("existing.md"),
            "---\nname: existing\ndescription: Old\n---\nOld.",
        )
        .unwrap();

        let files = vec![ImportFile {
            name: "existing.md".to_string(),
            content: "---\nname: existing\ndescription: New\n---\nNew.".to_string(),
            size_bytes: 45,
        }];

        install_skill_files(dir.path(), "existing", &files, true, "", "").unwrap();

        let content = std::fs::read_to_string(dir.path().join("existing.md")).unwrap();
        assert!(content.contains("New."));
    }

    #[test]
    fn test_provenance_content() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![ImportFile {
            name: "SKILL.md".to_string(),
            content: "---\nname: test\ndescription: Test\n---\nBody.".to_string(),
            size_bytes: 40,
        }];

        install_skill_files(
            dir.path(),
            "test",
            &files,
            false,
            "https://github.com/a/b",
            "sha789",
        )
        .unwrap();

        let source_path = dir.path().join("test").join(".source.json");
        let source_content = std::fs::read_to_string(&source_path).unwrap();
        let source: serde_json::Value = serde_json::from_str(&source_content).unwrap();
        assert_eq!(source["url"], "https://github.com/a/b");
        assert_eq!(source["commit_sha"], "sha789");
        assert_eq!(source["file_count"], 1);
        assert!(source["installed_at"].as_str().unwrap().len() > 10);
    }

    #[test]
    fn test_provenance_not_loaded_as_skill() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            ImportFile {
                name: "SKILL.md".to_string(),
                content: "---\nname: test-prov\ndescription: Test\n---\nBody.".to_string(),
                size_bytes: 45,
            },
            ImportFile {
                name: "extra.md".to_string(),
                content: "Extra.".to_string(),
                size_bytes: 6,
            },
        ];

        install_skill_files(
            dir.path(),
            "test-prov",
            &files,
            false,
            "https://example.com",
            "abc",
        )
        .unwrap();

        let registry = crate::runtime::skills::SkillRegistry::new(dir.path().to_path_buf());
        let skill = registry.get("test-prov").unwrap();
        assert!(skill.is_directory);
        assert_eq!(skill.files.len(), 1);
        assert_eq!(skill.files[0], "extra.md");
    }

    #[test]
    fn test_provenance_single_file_not_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![ImportFile {
            name: "my-skill.md".to_string(),
            content: "---\nname: my-skill\ndescription: Test\n---\nBody.".to_string(),
            size_bytes: 45,
        }];

        install_skill_files(
            dir.path(),
            "my-skill",
            &files,
            false,
            "https://example.com",
            "abc",
        )
        .unwrap();

        // .source-my-skill.json is dot-prefixed, should not load as skill
        let registry = crate::runtime::skills::SkillRegistry::new(dir.path().to_path_buf());
        assert!(registry.get("my-skill").is_some());
        assert!(registry.get(".source-my-skill").is_none());
        assert_eq!(registry.list().len(), 1);
    }

    // -- URL parser tests --

    #[test]
    fn test_parse_github_tree_url() {
        let result = parse_skill_url(
            "https://github.com/anthropics/skills/tree/main/skills/frontend-design",
        )
        .unwrap();
        assert_eq!(result.owner, "anthropics");
        assert_eq!(result.repo, "skills");
        assert_eq!(result.branch, "main");
        assert_eq!(result.path, "skills/frontend-design");
    }

    #[test]
    fn test_parse_github_blob_url() {
        let result =
            parse_skill_url("https://github.com/owner/repo/blob/main/skills/my-skill/SKILL.md")
                .unwrap();
        assert_eq!(result.owner, "owner");
        assert_eq!(result.repo, "repo");
        assert_eq!(result.branch, "main");
        assert_eq!(result.path, "skills/my-skill");
    }

    #[test]
    fn test_parse_skills_sh_url() {
        let result =
            parse_skill_url("https://skills.sh/anthropics/skills/frontend-design").unwrap();
        assert_eq!(result.owner, "anthropics");
        assert_eq!(result.repo, "skills");
        assert_eq!(result.branch, "main");
        assert_eq!(result.path, "skills/frontend-design");
    }

    #[test]
    fn test_parse_bare_repo_url() {
        let result = parse_skill_url("https://github.com/owner/my-skill").unwrap();
        assert_eq!(result.owner, "owner");
        assert_eq!(result.repo, "my-skill");
        assert_eq!(result.branch, "main");
        assert_eq!(result.path, ".");
    }

    #[test]
    fn test_parse_trailing_slash_stripped() {
        let result = parse_skill_url("https://github.com/owner/repo/").unwrap();
        assert_eq!(result.owner, "owner");
        assert_eq!(result.repo, "repo");
    }

    #[test]
    fn test_parse_git_suffix_stripped() {
        let result = parse_skill_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(result.owner, "owner");
        assert_eq!(result.repo, "repo");
    }

    #[test]
    fn test_parse_invalid_url() {
        assert!(parse_skill_url("not-a-url").is_err());
    }

    #[test]
    fn test_parse_unsupported_host() {
        assert!(parse_skill_url("https://gitlab.com/owner/repo").is_err());
    }

    #[test]
    fn test_path_prefix_boundary() {
        assert!(path_matches_skill_dir(
            "skills/frontend-design/SKILL.md",
            "skills/frontend-design"
        ));
        assert!(path_matches_skill_dir(
            "skills/frontend-design/tests.md",
            "skills/frontend-design"
        ));
        assert!(!path_matches_skill_dir(
            "skills/frontend-design-v2/SKILL.md",
            "skills/frontend-design"
        ));
        assert!(!path_matches_skill_dir(
            "skills/frontend/SKILL.md",
            "skills/frontend-design"
        ));
    }

    #[test]
    fn test_path_matches_root() {
        assert!(path_matches_skill_dir("SKILL.md", "."));
        assert!(path_matches_skill_dir("README.md", "."));
        assert!(!path_matches_skill_dir("subdir/file.md", "."));
    }
}
