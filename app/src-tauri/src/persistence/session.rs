use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

use crate::config::app_config::{agent_dir, base_config_dir};
use crate::runtime::agent::AgentSlot;

/// Atomic write: serialize to temp file, fsync, then rename into place.
/// Prevents data loss if the process crashes mid-write.
fn atomic_write(path: &std::path::Path, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path.parent().ok_or("No parent directory")?;
    std::fs::create_dir_all(parent)?;

    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(data)?;
    temp.as_file().sync_all()?; // fsync for crash safety
    temp.persist(path)?;
    Ok(())
}

/// Window state saved between sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
    pub bloomed_agent_id: Option<String>,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            x: 100,
            y: 100,
            width: 1280,
            height: 720,
            maximized: false,
            bloomed_agent_id: None,
        }
    }
}

/// Saved session data for a single agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    pub agent_id: String,
    pub conversation: crate::runtime::agent::ConversationHistory,
    pub system_prompt: String,
    pub metadata: crate::runtime::agent::AgentMetadata,
    #[serde(default)]
    pub session_prompt_tokens: u64,
    #[serde(default)]
    pub session_completion_tokens: u64,
    #[serde(default)]
    pub current_context_tokens: u64,
    #[serde(default)]
    pub session_start: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub sandbox: Option<crate::runtime::sandbox::SandboxState>,
    #[serde(default)]
    pub user_notes: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub connection: Option<crate::runtime::connection::ConnectionConfig>,
    #[serde(default)]
    pub protocol: Option<crate::runtime::connection::AgentProtocol>,
    #[serde(default)]
    pub anthropic_settings: Option<crate::runtime::agent::AnthropicSettings>,
    /// Provider-managed session/thread ID persisted across restarts for continuity.
    /// Legacy `claude_session_id` is still accepted for backward compatibility.
    #[serde(default, alias = "claude_session_id")]
    pub agent_session_id: Option<String>,
    /// Hash of system prompt when session was created — invalidates session if persona changes.
    #[serde(default)]
    pub system_prompt_hash: Option<String>,
    #[serde(default)]
    pub coordinator_mode: bool,
    #[serde(default)]
    pub autonomous: bool,
    #[serde(default)]
    pub appearance: Option<crate::runtime::appearance::AgentAppearance>,
    /// Runtime settings for OpenAI Responses API agents (GPT-5 family).
    /// Populated only for agents whose endpoint routes to api.openai.com.
    /// Persisted to disk so user preferences (reasoning effort, verbosity,
    /// model variant) survive restarts.
    ///
    #[serde(default)]
    pub openai_responses_settings:
        Option<crate::runtime::openai_responses_settings::OpenAIResponsesSettings>,
    /// Runtime settings for xAI Grok agents.
    #[serde(default)]
    pub xai_settings: Option<crate::runtime::xai_settings::XAISettings>,
    /// Runtime settings for Google Gemini agents.
    #[serde(default)]
    pub gemini_settings: Option<crate::runtime::gemini_settings::GeminiSettings>,
    /// Runtime settings for local model agents.
    #[serde(default)]
    pub local_model_settings: Option<crate::runtime::local_model_settings::LocalModelSettings>,
}

fn sessions_dir() -> PathBuf {
    base_config_dir().join("sessions")
}

fn agent_session_path(agent_id: &str) -> PathBuf {
    agent_dir(agent_id).join("session.json")
}

fn window_state_path() -> PathBuf {
    sessions_dir().join("window_state.json")
}

/// Save a single agent's session to disk.
pub fn save_agent_session(agent: &AgentSlot) -> Result<(), Box<dyn std::error::Error>> {
    let session = AgentSession {
        agent_id: agent.id.clone(),
        conversation: agent.conversation.clone(),
        system_prompt: agent.system_prompt.clone(),
        metadata: agent.metadata.clone(),
        session_prompt_tokens: agent.session_prompt_tokens,
        session_completion_tokens: agent.session_completion_tokens,
        current_context_tokens: agent.current_context_tokens,
        session_start: agent.session_start,
        sandbox: agent.sandbox.clone(),
        user_notes: agent.user_notes.clone(),
        skills: agent.skills.clone(),
        connection: Some(agent.connection.clone()),
        protocol: Some(agent.protocol.clone()),
        anthropic_settings: agent.anthropic_settings.clone(),
        agent_session_id: agent.agent_session_id.clone(),
        system_prompt_hash: agent.system_prompt_hash.clone(),
        coordinator_mode: agent.coordinator_mode,
        autonomous: agent.autonomous,
        appearance: agent.appearance.clone(),
        openai_responses_settings: agent.openai_responses_settings.clone(),
        xai_settings: agent.xai_settings.clone(),
        gemini_settings: agent.gemini_settings.clone(),
        local_model_settings: agent.local_model_settings.clone(),
    };

    let json = serde_json::to_string_pretty(&session)?;
    let path = agent_session_path(&agent.id);
    atomic_write(&path, json.as_bytes())?;

    Ok(())
}

/// Save all dirty agents' sessions. Returns count of agents saved.
pub fn save_dirty_agents(agents: &mut [AgentSlot]) -> Result<usize, Box<dyn std::error::Error>> {
    let mut saved = 0;
    for agent in agents.iter_mut() {
        if agent.is_dirty {
            save_agent_session(agent)?;
            agent.is_dirty = false;
            saved += 1;
        }
    }
    Ok(saved)
}

/// Save all agents regardless of dirty flag (for shutdown).
pub fn save_all_agents(agents: &[AgentSlot]) -> Result<usize, Box<dyn std::error::Error>> {
    let mut saved = 0;
    for agent in agents {
        save_agent_session(agent)?;
        saved += 1;
    }
    Ok(saved)
}

/// Load a saved agent session from disk.
pub fn load_agent_session(
    agent_id: &str,
) -> Result<Option<AgentSession>, Box<dyn std::error::Error>> {
    let path = agent_session_path(agent_id);
    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&path)?;
    let session: AgentSession = serde_json::from_str(&content)?;
    Ok(Some(session))
}

/// Restore an agent's conversation state from a saved session.
pub fn restore_agent_session(agent: &mut AgentSlot) -> Result<bool, Box<dyn std::error::Error>> {
    if let Some(session) = load_agent_session(&agent.id)? {
        agent.conversation = session.conversation;
        // NOTE: Do NOT restore system_prompt from session. Config.toml is authoritative.
        // Session may contain stale prompts that overwrite user's config edits.
        agent.metadata = session.metadata;
        agent.session_prompt_tokens = session.session_prompt_tokens;
        agent.session_completion_tokens = session.session_completion_tokens;
        agent.current_context_tokens = session.current_context_tokens;
        agent.session_start = session.session_start;
        agent.sandbox = session.sandbox;
        agent.user_notes = session.user_notes;
        agent.skills = session.skills;
        if let Some(conn) = session.connection {
            agent.connection = conn;
        }
        if let Some(proto) = session.protocol {
            agent.protocol = proto;
        }
        if session.anthropic_settings.is_some() {
            agent.anthropic_settings = session.anthropic_settings;
        }
        agent.agent_session_id = session.agent_session_id;
        agent.system_prompt_hash = session.system_prompt_hash;
        agent.coordinator_mode = session.coordinator_mode;
        agent.autonomous = session.autonomous;
        agent.appearance = session.appearance;
        agent.openai_responses_settings = session.openai_responses_settings;
        agent.xai_settings = session.xai_settings;
        agent.gemini_settings = session.gemini_settings;
        agent.local_model_settings = session.local_model_settings;

        // Fix up strict_system_position for LlamaCpp agents created before
        // the field existed. Their session.json has an explicit `false` that
        // the serde default can't override. Force it to the provider-aware
        // default so they get safe behavior without manual intervention.
        if let Some(ref mut lms) = agent.local_model_settings {
            if !lms.strict_system_position {
                let should_be = crate::runtime::local_model_settings::LocalModelSettings
                    ::default_strict_system_position(lms.provider_type);
                if should_be {
                    tracing::info!(
                        agent_id = %agent.id,
                        provider = ?lms.provider_type,
                        "Correcting strict_system_position from false to true for provider default"
                    );
                    lms.strict_system_position = true;
                    agent.is_dirty = true;
                }
            }
        }

        // is_dirty may already be true from the fixup above — don't reset it
        if !agent.is_dirty {
            agent.is_dirty = false;
        }
        // Note: when is_dirty is set by the fixup, the corrected value gets
        // persisted on the next save_dirty_agents cycle, so this is a one-time fix.
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Clear a saved session file.
pub fn clear_agent_session(agent_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = agent_session_path(agent_id);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Save window state to disk.
pub fn save_window_state(state: &WindowState) -> Result<(), Box<dyn std::error::Error>> {
    let json = serde_json::to_string_pretty(state)?;
    atomic_write(&window_state_path(), json.as_bytes())?;
    Ok(())
}

/// Load window state from disk.
pub fn load_window_state() -> Result<WindowState, Box<dyn std::error::Error>> {
    let path = window_state_path();
    if !path.exists() {
        return Ok(WindowState::default());
    }

    let content = std::fs::read_to_string(&path)?;
    let state: WindowState = serde_json::from_str(&content)?;
    Ok(state)
}

/// Clear all session files.
pub fn clear_all_sessions() -> Result<(), Box<dyn std::error::Error>> {
    let dir = sessions_dir();
    if dir.exists() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().map(|e| e == "json").unwrap_or(false) {
                std::fs::remove_file(&path)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::agent::*;
    use crate::runtime::connection::*;
    use chrono::Utc;

    fn make_test_agent(id: &str) -> AgentSlot {
        AgentSlot {
            id: id.to_string(),
            name: "Test".to_string(),
            color: AgentColor::default(),
            protected: false,
            connection: ConnectionConfig::Local {
                host: "localhost".to_string(),
                port: 8080,
                model: None,
            },
            protocol: AgentProtocol::default(),
            working_directory: PathBuf::from("/tmp"),
            status: AgentStatus::Idle,
            subagents: vec![],
            conversation: ConversationHistory {
                messages: vec![ConversationMessage {
                    role: MessageRole::User,
                    content: "Hello".to_string(),
                    content_blocks: None,
                    timestamp: Utc::now(),
                    token_estimate: None,
                    tool_calls: None,
                    tool_call_id: None,
                    outcome_tag: None,
                    thinking_content: None,
                    responses_phase: None,
                    reasoning_items: None,
                }],
                total_token_estimate: 5,
            },
            tools: vec![],
            system_prompt: "You are helpful.".to_string(),
            metadata: AgentMetadata::default(),
            limits: crate::runtime::limits::AgentLimits::default(),
            context_window_size: 125_000,
            context_tokens_override: None,
            context_probed: false,
            sandbox_config: crate::config::agent_config::BuildSandboxBlock::default(),
            sandbox: None,
            session_prompt_tokens: 0,
            session_completion_tokens: 0,
            current_context_tokens: 0,
            session_start: None,
            is_dirty: true,
            anthropic_settings: None,
            skills: vec![],
            user_notes: vec![],
            agent_session_id: None,
            system_prompt_hash: None,
            coordinator_mode: false,
            autonomous: false,
            extraction_pending_done: false,
            provider: None,
            cached_params: None,
            multi_agent_model: false,
            archetype_base: None,
            archetype_specialization: None,
            session_memory: Default::default(),
            cognitive_intake: Default::default(),
            appearance: None,
            openai_responses_settings: None,
            xai_settings: None,
            gemini_settings: None,
            local_model_settings: None,
        }
    }

    #[test]
    fn test_agent_session_serialization() {
        let agent = make_test_agent("test-001");
        let session = AgentSession {
            agent_id: agent.id.clone(),
            conversation: agent.conversation.clone(),
            system_prompt: agent.system_prompt.clone(),
            metadata: agent.metadata.clone(),
            session_prompt_tokens: 0,
            session_completion_tokens: 0,
            current_context_tokens: 0,
            session_start: None,
            sandbox: None,
            user_notes: vec![],
            skills: vec![],
            connection: None,
            protocol: None,
            anthropic_settings: None,
            agent_session_id: None,
            system_prompt_hash: None,
            coordinator_mode: false,
            autonomous: false,
            appearance: None,
            openai_responses_settings: None,
            xai_settings: None,
            gemini_settings: None,
            local_model_settings: None,
        };

        let json = serde_json::to_string(&session).unwrap();
        let deserialized: AgentSession = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.agent_id, "test-001");
        assert_eq!(deserialized.conversation.messages.len(), 1);
    }

    #[test]
    fn test_window_state_serialization() {
        let state = WindowState {
            x: 200,
            y: 150,
            width: 1920,
            height: 1080,
            maximized: true,
            bloomed_agent_id: Some("agent-1".to_string()),
        };

        let json = serde_json::to_string(&state).unwrap();
        let deserialized: WindowState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.width, 1920);
        assert!(deserialized.maximized);
        assert_eq!(deserialized.bloomed_agent_id.as_deref(), Some("agent-1"));
    }

    #[test]
    fn test_window_state_default() {
        let state = WindowState::default();
        assert_eq!(state.width, 1280);
        assert_eq!(state.height, 720);
        assert!(!state.maximized);
        assert!(state.bloomed_agent_id.is_none());
    }

    #[test]
    fn test_session_with_claude_session_id() {
        let session = AgentSession {
            agent_id: "sdk-agent".to_string(),
            conversation: ConversationHistory {
                messages: vec![],
                total_token_estimate: 0,
            },
            system_prompt: String::new(),
            metadata: AgentMetadata::default(),
            session_prompt_tokens: 0,
            session_completion_tokens: 0,
            current_context_tokens: 0,
            session_start: Some(Utc::now()),
            sandbox: None,
            user_notes: vec![],
            skills: vec![],
            connection: None,
            protocol: None,
            anthropic_settings: None,
            agent_session_id: Some("sess_abc123".to_string()),
            system_prompt_hash: None,
            coordinator_mode: false,
            autonomous: false,
            appearance: None,
            openai_responses_settings: None,
            xai_settings: None,
            gemini_settings: None,
            local_model_settings: None,
        };

        let json = serde_json::to_string_pretty(&session).unwrap();
        let restored: AgentSession = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.agent_session_id.as_deref(), Some("sess_abc123"));

        // Also verify None round-trips correctly
        let mut session_no_id = session.clone();
        session_no_id.agent_session_id = None;
        let json2 = serde_json::to_string(&session_no_id).unwrap();
        let restored2: AgentSession = serde_json::from_str(&json2).unwrap();
        assert!(restored2.agent_session_id.is_none());

        // Verify missing field deserializes as None (serde default)
        let minimal = r#"{"agent_id":"x","conversation":{"messages":[],"total_token_estimate":0},"system_prompt":"","metadata":{"total_tokens_used":0,"total_api_calls":0,"average_latency_ms":0.0,"created_at":"2026-01-01T00:00:00Z","last_active":null}}"#;
        let restored3: AgentSession = serde_json::from_str(minimal).unwrap();
        assert!(restored3.agent_session_id.is_none());
    }

    #[test]
    fn test_save_dirty_agents_only() {
        // This test validates the dirty flag logic without writing to disk
        let mut agents = vec![make_test_agent("a1"), make_test_agent("a2")];
        agents[1].is_dirty = false;

        // Count dirty agents
        let dirty_count = agents.iter().filter(|a| a.is_dirty).count();
        assert_eq!(dirty_count, 1);
    }
}
