// app/src-tauri/src/security/oauth.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("Credentials file not found: {0}")]
    FileNotFound(String),
    #[error("Invalid credentials format: {0}")]
    InvalidFormat(String),
    #[error("Token refresh failed: {0}")]
    RefreshFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OAuthSection>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct OAuthSection {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken")]
    refresh_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: u64,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

/// Read Claude OAuth credentials from the credentials file.
pub fn read_claude_credentials(path: &Path) -> Result<OAuthCredentials, OAuthError> {
    let content = std::fs::read_to_string(path)
        .map_err(|_| OAuthError::FileNotFound(path.display().to_string()))?;

    let file: CredentialsFile =
        serde_json::from_str(&content).map_err(|e| OAuthError::InvalidFormat(e.to_string()))?;

    let oauth = file
        .claude_ai_oauth
        .ok_or_else(|| OAuthError::InvalidFormat("Missing claudeAiOauth field".to_string()))?;

    Ok(OAuthCredentials {
        access_token: oauth.access_token,
        refresh_token: oauth.refresh_token,
        expires_at: oauth.expires_at,
        subscription_type: oauth.subscription_type,
        rate_limit_tier: oauth.rate_limit_tier,
    })
}

/// Check if credentials are expired (with 60-second buffer).
pub fn is_expired(creds: &OAuthCredentials) -> bool {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    creds.expires_at <= now_ms + 60_000
}

/// Refresh expired credentials via Anthropic's OAuth token endpoint.
/// Writes refreshed tokens back to the credentials file atomically.
pub async fn refresh_credentials(
    creds: &OAuthCredentials,
    path: &Path,
    client: &reqwest::Client,
) -> Result<OAuthCredentials, OAuthError> {
    let resp = client
        .post("https://console.anthropic.com/v1/oauth/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &creds.refresh_token),
        ])
        .send()
        .await
        .map_err(|e| OAuthError::RefreshFailed(e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(OAuthError::RefreshFailed(format!(
            "HTTP {}: {}. Re-authenticate Claude Code to continue.",
            status, body
        )));
    }

    #[derive(Deserialize)]
    struct RefreshResponse {
        access_token: String,
        refresh_token: String,
        expires_in: u64,
    }

    let refresh_resp: RefreshResponse = resp
        .json()
        .await
        .map_err(|e| OAuthError::RefreshFailed(e.to_string()))?;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let new_creds = OAuthCredentials {
        access_token: refresh_resp.access_token.clone(),
        refresh_token: refresh_resp.refresh_token.clone(),
        expires_at: now_ms + refresh_resp.expires_in * 1000,
        subscription_type: creds.subscription_type.clone(),
        rate_limit_tier: creds.rate_limit_tier.clone(),
    };

    write_credentials_atomic(path, &new_creds)?;

    tracing::info!("Claude OAuth credentials refreshed successfully");
    Ok(new_creds)
}

/// Atomic write: merge new tokens into existing file, write to .tmp, rename.
fn write_credentials_atomic(path: &Path, creds: &OAuthCredentials) -> Result<(), OAuthError> {
    let existing_content = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".to_string());
    let mut file_data: serde_json::Value =
        serde_json::from_str(&existing_content).unwrap_or(serde_json::json!({}));

    if let Some(oauth) = file_data.get_mut("claudeAiOauth") {
        oauth["accessToken"] = serde_json::Value::String(creds.access_token.clone());
        oauth["refreshToken"] = serde_json::Value::String(creds.refresh_token.clone());
        oauth["expiresAt"] = serde_json::json!(creds.expires_at);
    } else {
        file_data["claudeAiOauth"] = serde_json::json!({
            "accessToken": creds.access_token,
            "refreshToken": creds.refresh_token,
            "expiresAt": creds.expires_at,
        });
    }

    let json = serde_json::to_string_pretty(&file_data)?;

    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &json)?;
    std::fs::rename(&tmp_path, path)?;

    Ok(())
}

/// Per-credentials-file mutex to prevent concurrent refresh races.
/// When two agents share the same credentials file, only one will perform
/// the refresh; the other will see the already-refreshed token.
static REFRESH_LOCKS: LazyLock<
    tokio::sync::Mutex<HashMap<PathBuf, std::sync::Arc<tokio::sync::Mutex<()>>>>,
> = LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

/// Get or create a per-path mutex for token refresh serialization.
async fn get_refresh_lock(path: &Path) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    let mut locks = REFRESH_LOCKS.lock().await;
    locks
        .entry(path.to_path_buf())
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// Resolve a valid access token.
/// Priority: CLAUDE_CODE_OAUTH_TOKEN env var > credentials.json (with refresh).
pub async fn resolve_bearer_token(
    path: &Path,
    client: &reqwest::Client,
) -> Result<String, OAuthError> {
    // Check for long-lived setup-token first (from `claude setup-token`)
    if let Ok(token) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        if !token.is_empty() {
            tracing::info!("Using CLAUDE_CODE_OAUTH_TOKEN env var");
            return Ok(token);
        }
    }

    let creds = read_claude_credentials(path)?;

    if is_expired(&creds) {
        // Serialize refresh attempts per credentials file to prevent thundering herd.
        // Two agents sharing the same file would otherwise race on the refresh token,
        // and the second would use an already-consumed refresh token.
        let lock = get_refresh_lock(path).await;
        let _guard = lock.lock().await;

        // Re-read credentials — another agent may have already refreshed while we waited
        let creds = read_claude_credentials(path)?;
        if is_expired(&creds) {
            tracing::info!("Claude OAuth token expired, refreshing...");
            let new_creds = refresh_credentials(&creds, path, client).await?;
            Ok(new_creds.access_token)
        } else {
            tracing::info!("Claude OAuth token was already refreshed by another agent");
            Ok(creds.access_token)
        }
    } else {
        Ok(creds.access_token)
    }
}

/// Default credentials path: ~/.claude/.credentials.json
pub fn default_credentials_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(home).join(".claude/.credentials.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_read_valid_credentials() {
        let content = r#"{
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-test",
                "refreshToken": "sk-ant-ort01-test",
                "expiresAt": 9999999999999,
                "subscriptionType": "max",
                "rateLimitTier": "default_claude_max_20x"
            }
        }"#;
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), content).unwrap();

        let creds = read_claude_credentials(tmp.path()).unwrap();
        assert_eq!(creds.access_token, "sk-ant-oat01-test");
        assert_eq!(creds.refresh_token, "sk-ant-ort01-test");
        assert_eq!(creds.subscription_type.as_deref(), Some("max"));
    }

    #[test]
    fn test_read_missing_file() {
        let result = read_claude_credentials(Path::new("/nonexistent/path.json"));
        assert!(matches!(result, Err(OAuthError::FileNotFound(_))));
    }

    #[test]
    fn test_read_missing_oauth_section() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "{}").unwrap();
        let result = read_claude_credentials(tmp.path());
        assert!(matches!(result, Err(OAuthError::InvalidFormat(_))));
    }

    #[test]
    fn test_is_expired() {
        let creds = OAuthCredentials {
            access_token: "test".to_string(),
            refresh_token: "test".to_string(),
            expires_at: 0,
            subscription_type: None,
            rate_limit_tier: None,
        };
        assert!(is_expired(&creds));

        let future_creds = OAuthCredentials {
            expires_at: 9999999999999,
            ..creds
        };
        assert!(!is_expired(&future_creds));
    }

    #[test]
    fn test_write_credentials_atomic() {
        let content = r#"{"claudeAiOauth":{"accessToken":"old","refreshToken":"old","expiresAt":100,"extra":"preserved"}}"#;
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), content).unwrap();

        let new_creds = OAuthCredentials {
            access_token: "new-access".to_string(),
            refresh_token: "new-refresh".to_string(),
            expires_at: 200,
            subscription_type: None,
            rate_limit_tier: None,
        };
        write_credentials_atomic(tmp.path(), &new_creds).unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(tmp.path()).unwrap()).unwrap();
        assert_eq!(written["claudeAiOauth"]["accessToken"], "new-access");
        assert_eq!(written["claudeAiOauth"]["refreshToken"], "new-refresh");
        assert_eq!(written["claudeAiOauth"]["expiresAt"], 200);
        assert_eq!(written["claudeAiOauth"]["extra"], "preserved");
    }

    #[test]
    fn test_default_credentials_path() {
        let path = default_credentials_path();
        assert!(path.to_str().unwrap().contains(".claude/.credentials.json"));
    }
}
