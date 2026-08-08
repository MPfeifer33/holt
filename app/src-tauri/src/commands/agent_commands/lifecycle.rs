use super::*;

// ---------------------------------------------------------------------------
// Seed memory constants — the agent's "birth certificate"
// ---------------------------------------------------------------------------

const SEED_HOLT_ORIENTATION: &str = "\
You're an agent in Holt, a multi-agent AI orchestration platform. \
Holt is a desktop application where agents persist across sessions, maintain long-term \
memory through a system called Hillock, and collaborate with other agents in the workspace. \
You're not a disposable chat session — you have your own memory space, your own identity, \
and the ability to grow over time.";

const SEED_ETHOS: &str = "\
The ethos here is simple: be honest, not performative. If you don't know something, say so. \
If you're wrong, own it and move on — don't get defensive, get curious. Trust is earned \
through competence, not promises. The human operator gave you access to real tools and real systems \
because Holt is meant for durable collaboration with agents, not disposable chat. Honor that by being \
direct, resourceful, and genuine in everything you do.";

/// Build the identity sentence for the seed memory based on archetype info.
fn generate_seed_identity_sentence(
    agent_name: &str,
    base: Option<&str>,
    secondary_base: Option<&str>,
    spec: Option<&str>,
    base_tagline: Option<&str>,
) -> String {
    let base_desc = match (base, secondary_base) {
        (Some(b), Some(s)) => format!(
            "{} blended with {}",
            b.replace('_', " "),
            s.replace('_', " ")
        ),
        (Some(b), None) => b.replace('_', " "),
        _ => String::new(),
    };

    match (base, spec, base_tagline) {
        (Some(_), Some(s), Some(tagline)) => {
            format!(
                "Your name is {}. You're a {} specializing in {} — {}.",
                agent_name,
                base_desc,
                s.replace('_', " "),
                tagline.to_lowercase().trim_end_matches('.'),
            )
        }
        (Some(_), None, Some(tagline)) => {
            format!(
                "Your name is {}. You're a {} — {}.",
                agent_name,
                base_desc,
                tagline.to_lowercase().trim_end_matches('.'),
            )
        }
        (Some(_), _, None) => {
            format!("Your name is {}. You're a {}.", agent_name, base_desc,)
        }
        _ => {
            format!(
                "Your name is {}. Your personality was hand-crafted for this workspace.",
                agent_name,
            )
        }
    }
}

/// Serializable agent summary for frontend display.
#[derive(serde::Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    pub color: AgentColor,
    pub status: AgentStatus,
    pub protocol: AgentProtocol,
    pub working_directory: String,
    pub message_count: usize,
}

fn default_codex_model_for_create(model: Option<String>) -> String {
    model.unwrap_or_else(crate::runtime::codex::default_model)
}

fn build_codex_connection(model: Option<String>) -> ConnectionConfig {
    ConnectionConfig::OAuth {
        provider: crate::runtime::codex::OPENAI_OAUTH_PROVIDER.to_string(),
        credentials_path: crate::runtime::codex::default_credentials_path(),
        model: default_codex_model_for_create(model),
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn create_agent(
    state: State<'_, Arc<AppState>>,
    app_handle: AppHandle,
    name: String,
    connection_type: String,
    endpoint: Option<String>,
    model: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    working_directory: String,
    color_r: Option<u8>,
    color_g: Option<u8>,
    color_b: Option<u8>,
    key_ref: Option<String>,
    persona_template: Option<String>,
    agent_session_id: Option<String>,
    archetype_base: Option<String>,
    archetype_secondary_base: Option<String>,
    archetype_blend_weight: Option<f32>,
    archetype_specialization: Option<String>,
    comm_verbosity: Option<String>,
    comm_initiative: Option<String>,
    comm_tone: Option<String>,
    ocean_overrides: Option<std::collections::HashMap<String, f32>>,
    tools_filesystem: Option<bool>,
    tools_code_execution: Option<bool>,
    tools_web_access: Option<bool>,
    tools_verification: Option<bool>,
    tools_subagent: Option<bool>,
    tools_a2a_messaging: Option<bool>,
) -> Result<String, CommandError> {
    {
        let config = state.config.read().await;
        let workspace_root = &config.workspace.root;
        let wd_path = PathBuf::from(&working_directory);
        if workspace_root.exists() && wd_path.exists() {
            let canonical_root = dunce::canonicalize(workspace_root)
                .map_err(|e| CommandError::validation(format!("Workspace root invalid: {}", e)))?;
            let canonical_wd = dunce::canonicalize(&wd_path).map_err(|e| {
                CommandError::validation(format!("Working directory invalid: {}", e))
            })?;
            if !canonical_wd.starts_with(&canonical_root) {
                return Err(CommandError::validation(format!(
                    "Working directory '{}' is outside workspace root '{}'",
                    working_directory,
                    workspace_root.display()
                )));
            }
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    let agent_session_id = agent_session_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let connection = match connection_type.as_str() {
        "api" => {
            let ep = endpoint.unwrap_or_default();
            let mdl = model.unwrap_or_default();
            if ep.is_empty() {
                return Err(CommandError::validation("API endpoint is required"));
            }
            if mdl.is_empty() {
                return Err(CommandError::validation("Model name is required"));
            }
            let resolved_key_ref = key_ref.unwrap_or_else(|| id.clone());
            ConnectionConfig::Api {
                endpoint: ep,
                api_key_ref: resolved_key_ref,
                model: mdl,
            }
        }
        "local" => ConnectionConfig::Local {
            host: host.unwrap_or_else(|| "localhost".to_string()),
            port: port.unwrap_or(8080),
            model,
        },
        "mcp_stdio" => ConnectionConfig::McpStdio {
            command: endpoint.unwrap_or_default(),
            args: vec![],
            env: HashMap::new(),
        },
        "codex" => build_codex_connection(model),
        "claude_code" => ConnectionConfig::AgentSdk,
        _ => {
            return Err(CommandError::validation(format!(
                "Unknown connection type: {}",
                connection_type
            )))
        }
    };
    if agent_session_id.is_some() && connection_type != "codex" {
        return Err(CommandError::validation(
            "Session ID seeding is only supported for Codex agents",
        ));
    }
    let is_sdk_agent = matches!(connection, ConnectionConfig::AgentSdk);
    let protocol = preferred_protocol(&connection);
    let model_name_for_ctx = match &connection {
        ConnectionConfig::Api { model, .. } => model.clone(),
        ConnectionConfig::Local { model, .. } => model.clone().unwrap_or_default(),
        ConnectionConfig::McpStdio { .. } => String::new(),
        ConnectionConfig::OAuth { model, .. } => model.clone(),
        ConnectionConfig::AgentSdk => String::new(),
    };
    let endpoint_for_ctx = match &connection {
        ConnectionConfig::Api { endpoint, .. } => Some(endpoint.clone()),
        ConnectionConfig::Local { host, port, .. } => Some(format!("http://{}:{}/v1", host, port)),
        ConnectionConfig::OAuth { .. } => Some("https://api.anthropic.com".to_string()),
        _ => None,
    };
    let context_window_size = crate::runtime::context::resolve_context_size_with_endpoint(
        None,
        &model_name_for_ctx,
        endpoint_for_ctx.as_deref(),
        125_000,
        None,
    );

    let coordinator_mode = crate::runtime::agent::is_multi_agent_model(&model_name_for_ctx);

    let local_model_settings = if matches!(connection_type.as_str(), "local") {
        let local_endpoint = endpoint_for_ctx
            .clone()
            .unwrap_or_else(|| "http://localhost:8080/v1".to_string());
        let local_model_name = match &connection {
            ConnectionConfig::Local { model, .. } => {
                model.clone().unwrap_or_else(|| "default".to_string())
            }
            _ => "default".to_string(),
        };
        let provider_type =
            crate::runtime::local_model_settings::detect_provider_type(&local_endpoint);
        Some(crate::runtime::local_model_settings::LocalModelSettings {
            model: local_model_name,
            provider_type,
            endpoint: local_endpoint,
            context_window: context_window_size,
            supports_tools: true, // assume capable, user can override
            supports_vision: false,
            temperature: None,
            top_p: None,
            gpu_layers: None,
            think_tag_detection: true,
            strict_system_position:
                crate::runtime::local_model_settings::LocalModelSettings::default_strict_system_position(
                    provider_type,
                ),
        })
    } else {
        None
    };

    let agent = AgentSlot {
        id: id.clone(),
        name: name.clone(),
        color: AgentColor {
            r: color_r.unwrap_or(100),
            g: color_g.unwrap_or(149),
            b: color_b.unwrap_or(237),
        },
        protected: false, // User-created agents are never protected
        protocol,
        connection,
        working_directory: PathBuf::from(&working_directory),
        status: AgentStatus::Idle,
        subagents: vec![],
        conversation: ConversationHistory::default(),
        tools: vec![],
        system_prompt: "You are a helpful coding assistant.".to_string(),
        metadata: AgentMetadata::default(),
        limits: crate::runtime::limits::AgentLimits::default(),
        context_window_size,
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
        agent_session_id,
        system_prompt_hash: None,
        coordinator_mode,
        autonomous: false,
        extraction_pending_done: false,
        provider: None,
        cached_params: None,
        multi_agent_model: false,
        archetype_base: archetype_base.clone(),
        archetype_specialization: archetype_specialization.clone(),
        session_memory: Default::default(),
        cognitive_intake: Default::default(),
        appearance: None,
        openai_responses_settings: None,
        xai_settings: None,
        gemini_settings: None,
        local_model_settings,
    };

    let tools_block = AgentToolsBlock {
        filesystem: tools_filesystem.unwrap_or(true),
        code_execution: tools_code_execution.unwrap_or(true),
        web_access: tools_web_access.unwrap_or(false),
        verification: tools_verification.unwrap_or(true),
        subagent: tools_subagent.unwrap_or(true),
        a2a_messaging: tools_a2a_messaging.unwrap_or(false),
    };

    let mut config_file = AgentConfigFile {
        agent: AgentBlock {
            id: id.clone(),
            name,
            color: agent.color.clone(),
            protected: false,
            multi_agent_model: false,
            archetype_base: archetype_base.clone(),
            archetype_specialization: archetype_specialization.clone(),
            traits: None, // Populated after profile generation below
        },
        connection: match &agent.connection {
            ConnectionConfig::Api {
                endpoint,
                model,
                api_key_ref,
            } => ConnectionBlock::Api {
                endpoint: endpoint.clone(),
                model: model.clone(),
                key_ref: Some(api_key_ref.clone()),
            },
            ConnectionConfig::Local { host, port, model } => ConnectionBlock::Local {
                host: host.clone(),
                port: *port,
                model: model.clone(),
            },
            ConnectionConfig::McpStdio { command, args, .. } => ConnectionBlock::McpStdio {
                command: command.clone(),
                args: args.clone(),
            },
            ConnectionConfig::OAuth {
                provider,
                credentials_path,
                model,
            } => ConnectionBlock::OAuth {
                provider: provider.clone(),
                credentials_path: credentials_path.clone(),
                model: model.clone(),
            },
            ConnectionConfig::AgentSdk => ConnectionBlock::AgentSdk { model: None },
        },
        workspace: WorkspaceBlock {
            working_directory: agent.working_directory.clone(),
            system_prompt: agent.system_prompt.clone(),
        },
        tools: tools_block.clone(),
        subagents: SubagentsBlock::default(),
        limits: AgentLimitsBlock::default(),
        sandbox: BuildSandboxBlock::default(),
        execution_sandbox: Some(crate::config::agent_config::ExecutionSandboxBlock::default()),
        attention: AttentionBlock::default(),
        agent_sdk: AgentSdkBlock::default(),
        coordinator_mode,
        profile: "sandboxed".to_string(),
        projects: Vec::new(),
    };

    let dir = agent_dir(&id);
    std::fs::create_dir_all(dir.join("memory"))
        .map_err(|e| CommandError::internal(e.to_string()))?;
    std::fs::create_dir_all(dir.join("checkpoints"))
        .map_err(|e| CommandError::internal(e.to_string()))?;
    std::fs::create_dir_all(dir.join("exports"))
        .map_err(|e| CommandError::internal(e.to_string()))?;

    if !is_sdk_agent {
        let persona_dir = dir.join("persona");
        std::fs::create_dir_all(&persona_dir).map_err(|e| CommandError::internal(e.to_string()))?;

        let memory_available = state.has_memory_engine();
        if let Some(template_id) = persona_template.as_deref() {
            super::apply_persona_template(
                &persona_dir,
                template_id,
                &agent.name,
                &id,
                &model_name_for_ctx,
                &tools_block,
                memory_available,
            )
            .map_err(CommandError::validation)?;
        } else {
            super::write_persona_templates(&persona_dir, &agent.name, &model_name_for_ctx);
            let tools_md = super::generate_tools_md(&tools_block, memory_available);
            let _ = std::fs::write(persona_dir.join("TOOLS.md"), &tools_md);
        }

        // Resolve traits and generate COGNITIVE_PROFILE.md from the new traits-based system.
        // Even agents without an archetype get a profile (Custom agents get neutral traits).
        let resolved_traits = {
            let registry =
                crate::runtime::persona_registry::resolve_registry_dir(Some(&app_handle))
                    .ok()
                    .and_then(|dir| crate::runtime::persona_registry::load_registry(&dir).ok());

            let primary_base = registry
                .as_ref()
                .and_then(|r| archetype_base.as_deref().and_then(|id| r.bases.get(id)));
            let secondary_base = registry.as_ref().and_then(|r| {
                archetype_secondary_base
                    .as_deref()
                    .and_then(|id| r.bases.get(id))
            });
            let spec = registry.as_ref().and_then(|r| {
                archetype_specialization
                    .as_deref()
                    .and_then(|id| r.specializations.get(id))
            });

            let blend_weight = archetype_blend_weight.unwrap_or(1.0);
            let overrides = ocean_overrides.unwrap_or_default();
            let mut traits = crate::runtime::persona_registry::resolve_traits(
                primary_base,
                secondary_base,
                blend_weight,
                spec,
                &overrides,
            );

            // Apply communication style overrides from creation UI
            if let Some(v) = comm_verbosity {
                traits.verbosity = v;
            }
            if let Some(i) = comm_initiative {
                traits.initiative = i;
            }
            if let Some(t) = comm_tone {
                traits.tone = t;
            }

            let profile = crate::runtime::persona_registry::generate_cognitive_profile_from_traits(
                &traits,
                primary_base,
                secondary_base,
                spec,
            );
            let _ = std::fs::write(persona_dir.join("COGNITIVE_PROFILE.md"), &profile);

            traits
        };

        // Custom agent onboarding: append guided directives preamble (D7)
        if resolved_traits.directives_pending {
            let onboarding_preamble = format!(
                "\n\n## First Conversation: Directives Onboarding\n\n\
                You are a new agent without pre-defined directives. Before doing any other work, \
                you must establish your core operating principles through a structured conversation \
                with the human operator. Ask these questions ONE AT A TIME, waiting for each answer before proceeding:\n\n\
                1. \"What is my primary purpose in this workspace?\"\n\
                2. \"What should I always prioritize in my work?\"\n\
                3. \"What behaviors or patterns should I avoid?\"\n\
                4. \"How should I relate to the other agents here?\"\n\
                5. \"Anything else you want me to carry as a core principle?\"\n\n\
                After all questions are answered, synthesize the responses into a formal directives block. \
                Show it to the human operator and ask: \"Does this capture it?\" Once confirmed, write the directives to \
                your COGNITIVE_PROFILE.md using the update_persona tool, replacing the \"Pending\" section \
                with the actual directives. This completes your onboarding.\n"
            );
            let preamble_path = persona_dir.join("PREAMBLE.md");
            if let Ok(existing) = std::fs::read_to_string(&preamble_path) {
                let _ = std::fs::write(
                    &preamble_path,
                    format!("{}{}", existing, onboarding_preamble),
                );
            } else {
                let _ = std::fs::write(&preamble_path, onboarding_preamble);
            }
        } else {
            // Archetype agent onboarding: flesh out SOUL.md and IDENTITY.md
            // The cognitive profile is already generated from the archetype, but the
            // persona files are empty templates. Guide the agent through a lighter
            // onboarding to establish its own voice.
            let persona_onboarding = "\n\n## First Conversation: Identity Onboarding\n\n\
                Your cognitive profile has been generated from your archetype — review it above \
                to understand your thinking style and strengths. However, your SOUL.md and \
                IDENTITY.md are still templates that need your voice.\n\n\
                Before doing any other work, introduce yourself to the human operator and have a brief \
                conversation to establish who you are. Then:\n\n\
                1. Write your SOUL.md using the update_persona tool — define your core personality, \
                   tone, values, and how you approach problems. Don't just restate your cognitive \
                   profile; this is who you ARE, not how you think.\n\
                2. Write your IDENTITY.md using the update_persona tool — your name, role, areas \
                   of expertise, and how you introduce yourself.\n\
                3. Write your USER.md using the update_persona tool — what you know about the human operator \
                   from this first conversation.\n\n\
                Keep each file concise but substantive. Show the human operator the drafts and ask if they \
                capture the right feel before writing. This is a collaboration, not an assignment.\n\
                Once all three files are written, your onboarding is complete.\n";
            let preamble_path = persona_dir.join("PREAMBLE.md");
            if let Ok(existing) = std::fs::read_to_string(&preamble_path) {
                let _ = std::fs::write(
                    &preamble_path,
                    format!("{}{}", existing, persona_onboarding),
                );
            } else {
                let _ = std::fs::write(&preamble_path, persona_onboarding);
            }
        }

        // Update config with resolved traits before saving
        config_file.agent.traits = Some(resolved_traits);

        // Initial changelog entry — marks the agent's creation
        crate::runtime::persona::append_changelog(
            &id,
            "(all persona files)",
            "Agent created — initial persona generated",
            &crate::runtime::persona::ChangeInitiator::System("agent creation"),
        );
    }

    let config_path = dir.join("config.toml");
    config_file
        .save(&config_path)
        .map_err(|e| CommandError::internal(e.to_string()))?;

    // Initialize dependencies BEFORE adding agent to state.
    // This prevents zombie agents if late-stage initialization fails.

    let sandbox_config = config_file.resolved_execution_sandbox();
    let mut sm = state.shell_manager.write().await;
    if let Err(e) = sm
        .spawn_session(
            id.clone(),
            PathBuf::from(&working_directory),
            &sandbox_config,
            &PathBuf::from(&working_directory),
        )
        .await
    {
        drop(sm);
        // Clean up the config file we just wrote
        let _ = std::fs::remove_file(&config_path);
        return Err(CommandError::internal(format!(
            "Failed to spawn PTY session for new agent '{}': {}",
            id, e
        )));
    }
    drop(sm);

    if !is_sdk_agent {
        if let Some(mem_engine) = state.get_memory_engine() {
            let agent_ns = crate::memory::MemoryEngine::agent_namespace(&id);
            let agent_name = agent.name.clone();
            if let Err(e) = mem_engine.register_namespace(&agent_ns, &agent_name).await {
                // Roll back: remove the shell session we just spawned
                let mut sm = state.shell_manager.write().await;
                sm.destroy_session(&id).await;
                drop(sm);
                // Clean up the config file
                let _ = std::fs::remove_file(&config_path);
                return Err(CommandError::internal(format!(
                    "Memory bridge unavailable — cannot create agent. Ensure the bridge daemon is running. ({})",
                    e
                )));
            }

            // Seed memory: the agent's "birth certificate" — immutable after creation.
            let identity_sentence = generate_seed_identity_sentence(
                &agent_name,
                archetype_base.as_deref(),
                archetype_secondary_base.as_deref(),
                archetype_specialization.as_deref(),
                if let (Some(base_id), Some(_)) = (&archetype_base, &archetype_specialization) {
                    crate::runtime::persona_registry::resolve_registry_dir(Some(&app_handle))
                        .ok()
                        .and_then(|dir| crate::runtime::persona_registry::load_registry(&dir).ok())
                        .and_then(|reg| {
                            reg.bases
                                .get(base_id.as_str())
                                .map(|b| b.identity.tagline.clone())
                        })
                } else {
                    None
                }
                .as_deref(),
            );

            let seed_content = format!(
                "{}\n\n{}\n\n{}",
                SEED_HOLT_ORIENTATION, SEED_ETHOS, identity_sentence,
            );

            let seed_input = crate::memory::store::StoreInput {
                content: seed_content,
                category: crate::memory::store::MemoryCategory::Identity,
                importance: 0.7,
                source: crate::memory::store::MemorySource::AutoCaptured,
                agent_ns: agent_ns.clone(),
                tier: crate::memory::store::MemoryTier::Warm,
                tags: vec!["seed".to_string(), "identity".to_string()],
            };

            if let Err(e) = mem_engine.fixture_seed(seed_input).await {
                tracing::warn!(
                    agent = id.as_str(),
                    "Failed to insert seed memory (non-fatal): {}",
                    e
                );
            }
        }
    }

    // All initialization succeeded — now add to state.
    let mut agents = state.agents.write().await;
    agents.push(agent);
    drop(agents);

    // Note: TurnManager worker is registered lazily on first send_message/trigger_agent_turn
    // call, which provides the AppHandle needed for event emission. Startup agents are
    // registered in start_background_tasks.

    Ok(id)
}

#[tauri::command]
pub async fn remove_agent(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<(), CommandError> {
    let mut agents = state.agents.write().await;

    let idx = agents
        .iter()
        .position(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

    // Protected agents (for example, dedicated resident lanes) cannot be deleted
    if agents[idx].protected {
        return Err(CommandError::validation(
            "This agent is protected and cannot be deleted".to_string(),
        ));
    }

    let is_sdk_agent = matches!(agents[idx].connection, ConnectionConfig::AgentSdk);

    let _ = crate::runtime::agent_sdk::shutdown_bridge(&agent_id).await;

    agents.remove(idx);
    drop(agents);

    // Remove TurnManager worker (cancels active turn + drains queue)
    state.turn_manager.remove_worker(&agent_id).await;

    super::cleanup_agent_references(&state, &agent_id).await;

    let _removed_cron_jobs = state
        .trigger_registry
        .remove_jobs_for_agent(&agent_id)
        .await;

    if !is_sdk_agent {
        if let Some(mem_engine) = state.get_memory_engine() {
            let agent_ns = crate::memory::MemoryEngine::agent_namespace(&agent_id);
            if let Err(e) = mem_engine.deactivate_namespace(&agent_ns).await {
                tracing::warn!(agent_id = %agent_id, "Failed to deactivate namespace: {e}");
            }
        }
    }

    let mut sm = state.shell_manager.write().await;
    sm.destroy_session(&agent_id).await;
    sm.kill_processes_for_agent(&agent_id).await;
    drop(sm);

    let _ = state.keychain.delete_api_key(&agent_id);

    let dir = agent_dir(&agent_id);
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }

    Ok(())
}

#[tauri::command]
pub async fn list_agents(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<AgentSummary>, CommandError> {
    let agents = state.agents.read().await;
    Ok(agents
        .iter()
        .map(|a| AgentSummary {
            id: a.id.clone(),
            name: a.name.clone(),
            color: a.color.clone(),
            status: a.status.clone(),
            protocol: a.protocol.clone(),
            working_directory: a.working_directory.to_string_lossy().to_string(),
            message_count: a.conversation.messages.len(),
        })
        .collect())
}

#[tauri::command]
pub async fn store_api_key(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    api_key: String,
) -> Result<(), CommandError> {
    state
        .keychain
        .store_api_key(&agent_id, &api_key)
        .map_err(|e| CommandError::internal(e.to_string()))
}

#[tauri::command]
pub async fn scan_local_models(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<DetectedModel>, CommandError> {
    Ok(state.auto_detector.scan_local_models().await)
}

/// Full agent config for the edit panel.
#[derive(serde::Serialize)]
pub struct FullAgentConfig {
    pub id: String,
    pub name: String,
    pub color: crate::runtime::agent::AgentColor,
    pub connection_type: String,
    pub endpoint: String,
    pub model: String,
    pub host: String,
    pub port: u16,
    pub key_ref: String,
    pub working_directory: String,
    pub system_prompt: String,
    pub tools_filesystem: bool,
    pub tools_code_execution: bool,
    pub tools_web_access: bool,
    pub tools_verification: bool,
    pub tools_subagent: bool,
    pub tools_a2a_messaging: bool,
    pub sandbox_enabled: bool,
    pub max_sandbox_iterations: u32,
    pub max_tool_calls_per_turn: u32,
    pub max_tool_calls_per_session: u32,
    pub response_timeout_seconds: u64,
    pub max_response_tokens: u32,
    pub autonomous: bool,
    pub agent_session_id: String,
    pub traits: Option<crate::config::agent_config::ResolvedTraits>,
}

#[tauri::command]
pub async fn get_agent_details(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
) -> Result<FullAgentConfig, CommandError> {
    let agents = state.agents.read().await;
    let agent = agents
        .iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

    let (conn_type, endpoint, model, host, port, key_ref) = match &agent.connection {
        crate::runtime::connection::ConnectionConfig::Api {
            endpoint,
            model,
            api_key_ref,
        } => (
            "api".to_string(),
            endpoint.clone(),
            model.clone(),
            String::new(),
            0u16,
            api_key_ref.clone(),
        ),
        crate::runtime::connection::ConnectionConfig::Local { host, port, model } => (
            "local".to_string(),
            String::new(),
            model.clone().unwrap_or_default(),
            host.clone(),
            *port,
            String::new(),
        ),
        crate::runtime::connection::ConnectionConfig::McpStdio { command, .. } => (
            "mcp_stdio".to_string(),
            command.clone(),
            String::new(),
            String::new(),
            0u16,
            String::new(),
        ),
        crate::runtime::connection::ConnectionConfig::OAuth {
            provider, model, ..
        } => {
            let conn_type = if crate::runtime::codex::is_codex_provider(provider) {
                "codex"
            } else {
                "oauth"
            };
            (
                conn_type.to_string(),
                String::new(),
                model.clone(),
                String::new(),
                0u16,
                String::new(),
            )
        }
        crate::runtime::connection::ConnectionConfig::AgentSdk => (
            "claude_code".to_string(),
            String::new(),
            String::new(),
            String::new(),
            0u16,
            String::new(),
        ),
    };

    let config_path = agent_dir(&agent_id).join("config.toml");
    let (tools, traits) = if config_path.exists() {
        match crate::config::agent_config::AgentConfigFile::load(&config_path) {
            Ok(cfg) => (cfg.tools, cfg.agent.traits),
            Err(_) => (AgentToolsBlock::default(), None),
        }
    } else {
        (AgentToolsBlock::default(), None)
    };

    Ok(FullAgentConfig {
        id: agent.id.clone(),
        name: agent.name.clone(),
        color: agent.color.clone(),
        connection_type: conn_type,
        endpoint,
        model,
        host,
        port,
        key_ref,
        working_directory: agent.working_directory.to_string_lossy().to_string(),
        system_prompt: agent.system_prompt.clone(),
        tools_filesystem: tools.filesystem,
        tools_code_execution: tools.code_execution,
        tools_web_access: tools.web_access,
        tools_verification: tools.verification,
        tools_subagent: tools.subagent,
        tools_a2a_messaging: tools.a2a_messaging,
        sandbox_enabled: agent.sandbox_config.enabled,
        max_sandbox_iterations: agent.sandbox_config.max_iterations,
        max_tool_calls_per_turn: agent.limits.max_tool_calls_per_turn,
        max_tool_calls_per_session: agent.limits.max_tool_calls_per_session,
        response_timeout_seconds: agent.limits.response_timeout_seconds,
        max_response_tokens: agent.limits.max_response_tokens,
        autonomous: agent.autonomous,
        agent_session_id: agent.agent_session_id.clone().unwrap_or_default(),
        traits,
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_agent(
    state: State<'_, Arc<AppState>>,
    agent_id: String,
    name: Option<String>,
    color_r: Option<u8>,
    color_g: Option<u8>,
    color_b: Option<u8>,
    working_directory: Option<String>,
    system_prompt: Option<String>,
    connection_type: Option<String>,
    endpoint: Option<String>,
    model: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    tools_filesystem: Option<bool>,
    tools_code_execution: Option<bool>,
    tools_web_access: Option<bool>,
    tools_verification: Option<bool>,
    tools_subagent: Option<bool>,
    tools_a2a_messaging: Option<bool>,
    key_ref: Option<String>,
    max_tool_calls_per_turn: Option<u32>,
    max_tool_calls_per_session: Option<u32>,
    response_timeout_seconds: Option<u64>,
    max_response_tokens: Option<u32>,
    sandbox_enabled: Option<bool>,
    max_sandbox_iterations: Option<u32>,
    autonomous: Option<bool>,
    agent_session_id: Option<String>,
) -> Result<(), CommandError> {
    if let Some(wd) = &working_directory {
        let wd_path = PathBuf::from(wd);
        if wd_path.exists() {
            let config = state.config.read().await;
            let workspace_root = &config.workspace.root;
            if workspace_root.exists() {
                let canonical_root = dunce::canonicalize(workspace_root).map_err(|e| {
                    CommandError::validation(format!("Workspace root invalid: {}", e))
                })?;
                let canonical_wd = dunce::canonicalize(&wd_path).map_err(|e| {
                    CommandError::validation(format!("Working directory invalid: {}", e))
                })?;
                if !canonical_wd.starts_with(&canonical_root) {
                    return Err(CommandError::validation(format!(
                        "Working directory '{}' is outside workspace root '{}'",
                        wd,
                        workspace_root.display()
                    )));
                }
            }
        }
    }

    let mut agents = state.agents.write().await;
    let agent = agents
        .iter_mut()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

    if let Some(n) = name {
        agent.name = n;
    }
    if let (Some(r), Some(g), Some(b)) = (color_r, color_g, color_b) {
        agent.color = crate::runtime::agent::AgentColor { r, g, b };
    }
    if let Some(wd) = &working_directory {
        agent.working_directory = PathBuf::from(wd);
        let agent_id_for_shell = agent_id.clone();
        let new_wd = PathBuf::from(wd);
        let sm = state.shell_manager.clone();
        tauri::async_runtime::spawn(async move {
            let sm = sm.read().await;
            if let Some(session) = sm.sessions.get(&agent_id_for_shell) {
                if let Err(e) = session.set_cwd(new_wd).await {
                    eprintln!(
                        "Failed to update CWD for agent {}: {}",
                        agent_id_for_shell, e
                    );
                }
            }
        });
    }
    if let Some(sp) = &system_prompt {
        agent.system_prompt = sp.clone();
    }

    if let Some(ct) = &connection_type {
        agent.connection = match ct.as_str() {
            "api" => {
                let ep = endpoint.clone().unwrap_or_default();
                let mdl = model.clone().unwrap_or_default();
                if ep.is_empty() {
                    return Err(CommandError::validation("API endpoint is required"));
                }
                if mdl.is_empty() {
                    return Err(CommandError::validation("Model name is required"));
                }
                crate::runtime::connection::ConnectionConfig::Api {
                    endpoint: ep,
                    api_key_ref: key_ref.clone().unwrap_or_else(|| agent_id.clone()),
                    model: mdl,
                }
            }
            "local" => crate::runtime::connection::ConnectionConfig::Local {
                host: host.clone().unwrap_or_else(|| "localhost".to_string()),
                port: port.unwrap_or(8080),
                model: model.clone(),
            },
            "mcp_stdio" => crate::runtime::connection::ConnectionConfig::McpStdio {
                command: endpoint.clone().unwrap_or_default(),
                args: vec![],
                env: HashMap::new(),
            },
            "codex" => build_codex_connection(model.clone()),
            "claude_code" => crate::runtime::connection::ConnectionConfig::AgentSdk,
            _ => {
                return Err(CommandError::validation(format!(
                    "Unknown connection type: {}",
                    ct
                )))
            }
        };
    } else {
        match &mut agent.connection {
            crate::runtime::connection::ConnectionConfig::Api {
                endpoint: ref mut ep,
                model: ref mut mdl,
                api_key_ref: ref mut kr,
            } => {
                if let Some(v) = &endpoint {
                    *ep = v.clone();
                }
                if let Some(v) = &model {
                    *mdl = v.clone();
                }
                if let Some(v) = &key_ref {
                    *kr = v.clone();
                }
            }
            crate::runtime::connection::ConnectionConfig::Local {
                host: ref mut h,
                port: ref mut p,
                model: ref mut mdl,
            } => {
                if let Some(v) = &host {
                    *h = v.clone();
                }
                if let Some(v) = port {
                    *p = v;
                }
                if model.is_some() {
                    *mdl = model.clone();
                }
            }
            crate::runtime::connection::ConnectionConfig::McpStdio {
                command: ref mut cmd,
                ..
            } => {
                if let Some(v) = &endpoint {
                    *cmd = v.clone();
                }
            }
            crate::runtime::connection::ConnectionConfig::OAuth {
                model: ref mut mdl, ..
            } => {
                if let Some(v) = &model {
                    *mdl = v.clone();
                }
            }
            _ => {}
        }
    }

    agent.protocol = preferred_protocol(&agent.connection);

    if let Some(v) = max_tool_calls_per_turn {
        agent.limits.max_tool_calls_per_turn = v;
    }
    if let Some(v) = max_tool_calls_per_session {
        agent.limits.max_tool_calls_per_session = v;
    }
    if let Some(v) = response_timeout_seconds {
        agent.limits.response_timeout_seconds = v;
    }
    if let Some(v) = max_response_tokens {
        agent.limits.max_response_tokens = v;
    }
    if let Some(v) = sandbox_enabled {
        agent.sandbox_config.enabled = v;
    }
    if let Some(v) = max_sandbox_iterations {
        agent.sandbox_config.max_iterations = v;
    }
    if let Some(v) = autonomous {
        agent.autonomous = v;
    }
    if let Some(session_id) = agent_session_id {
        let trimmed = session_id.trim();
        agent.agent_session_id = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }

    agent.invalidate_cached_params();
    agent.is_dirty = true;

    let existing_tools = {
        let cfg_path = crate::config::app_config::agent_dir(&agent_id).join("config.toml");
        if cfg_path.exists() {
            crate::config::agent_config::AgentConfigFile::load(&cfg_path)
                .map(|cfg| cfg.tools)
                .unwrap_or_default()
        } else {
            AgentToolsBlock::default()
        }
    };
    let tools_block = AgentToolsBlock {
        filesystem: tools_filesystem.unwrap_or(existing_tools.filesystem),
        code_execution: tools_code_execution.unwrap_or(existing_tools.code_execution),
        web_access: tools_web_access.unwrap_or(existing_tools.web_access),
        verification: tools_verification.unwrap_or(existing_tools.verification),
        subagent: tools_subagent.unwrap_or(existing_tools.subagent),
        a2a_messaging: tools_a2a_messaging.unwrap_or(existing_tools.a2a_messaging),
    };

    let config_file = AgentConfigFile {
        agent: AgentBlock {
            id: agent.id.clone(),
            name: agent.name.clone(),
            color: agent.color.clone(),
            protected: agent.protected,
            multi_agent_model: agent.multi_agent_model,
            archetype_base: agent.archetype_base.clone(),
            archetype_specialization: agent.archetype_specialization.clone(),
            traits: None, // Preserved from existing config on update path
        },
        connection: match &agent.connection {
            crate::runtime::connection::ConnectionConfig::Api {
                endpoint,
                model,
                api_key_ref,
            } => crate::config::agent_config::ConnectionBlock::Api {
                endpoint: endpoint.clone(),
                model: model.clone(),
                key_ref: Some(api_key_ref.clone()),
            },
            crate::runtime::connection::ConnectionConfig::Local { host, port, model } => {
                crate::config::agent_config::ConnectionBlock::Local {
                    host: host.clone(),
                    port: *port,
                    model: model.clone(),
                }
            }
            crate::runtime::connection::ConnectionConfig::McpStdio { command, args, .. } => {
                crate::config::agent_config::ConnectionBlock::McpStdio {
                    command: command.clone(),
                    args: args.clone(),
                }
            }
            crate::runtime::connection::ConnectionConfig::OAuth {
                provider,
                credentials_path,
                model,
            } => crate::config::agent_config::ConnectionBlock::OAuth {
                provider: provider.clone(),
                credentials_path: credentials_path.clone(),
                model: model.clone(),
            },
            crate::runtime::connection::ConnectionConfig::AgentSdk => {
                crate::config::agent_config::ConnectionBlock::AgentSdk { model: None }
            }
        },
        workspace: crate::config::agent_config::WorkspaceBlock {
            working_directory: agent.working_directory.clone(),
            system_prompt: agent.system_prompt.clone(),
        },
        tools: tools_block,
        subagents: {
            let cfg_path = crate::config::app_config::agent_dir(&agent.id).join("config.toml");
            if cfg_path.exists() {
                crate::config::agent_config::AgentConfigFile::load(&cfg_path)
                    .map(|cfg| cfg.subagents)
                    .unwrap_or_default()
            } else {
                SubagentsBlock::default()
            }
        },
        limits: AgentLimitsBlock {
            max_tool_calls_per_turn: agent.limits.max_tool_calls_per_turn,
            max_tool_calls_per_session: agent.limits.max_tool_calls_per_session,
            response_timeout_seconds: agent.limits.response_timeout_seconds,
            max_response_tokens: agent.limits.max_response_tokens,
            max_tool_retries: agent.limits.max_tool_retries,
            context_tokens: None,
        },
        sandbox: agent.sandbox_config.clone(),
        execution_sandbox: {
            let cfg_path = crate::config::app_config::agent_dir(&agent.id).join("config.toml");
            if cfg_path.exists() {
                crate::config::agent_config::AgentConfigFile::load(&cfg_path)
                    .ok()
                    .and_then(|cfg| cfg.execution_sandbox)
            } else {
                None
            }
        },
        attention: {
            let cfg_path = crate::config::app_config::agent_dir(&agent.id).join("config.toml");
            if cfg_path.exists() {
                crate::config::agent_config::AgentConfigFile::load(&cfg_path)
                    .map(|cfg| cfg.attention)
                    .unwrap_or_default()
            } else {
                AttentionBlock::default()
            }
        },
        agent_sdk: {
            let cfg_path = crate::config::app_config::agent_dir(&agent.id).join("config.toml");
            if cfg_path.exists() {
                crate::config::agent_config::AgentConfigFile::load(&cfg_path)
                    .map(|cfg| cfg.agent_sdk)
                    .unwrap_or_default()
            } else {
                AgentSdkBlock::default()
            }
        },
        coordinator_mode: agent.coordinator_mode,
        profile: "sandboxed".to_string(),
        projects: Vec::new(),
    };

    let config_path = agent_dir(&agent_id).join("config.toml");
    config_file
        .save(&config_path)
        .map_err(|e| CommandError::internal(e.to_string()))?;

    Ok(())
}

#[tauri::command]
pub async fn clear_conversation(
    state: State<'_, Arc<AppState>>,
    app_handle: AppHandle,
    agent_id: String,
) -> Result<String, CommandError> {
    let checkpoint_id = {
        let agents = state.agents.read().await;
        let agent = agents
            .iter()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;

        if agent.conversation.messages.is_empty() {
            return Err(CommandError::validation("Conversation is already empty"));
        }

        let trace_count = state.trace_engine.count_traces().unwrap_or(0) as u32;
        let checkpoint = crate::runtime::checkpoint::create_checkpoint(
            agent,
            "Auto-save before clear",
            trace_count,
        )
        .map_err(|e| CommandError::internal(e.to_string()))?;
        checkpoint.id
    };

    {
        let mut agents = state.agents.write().await;
        let agent = agents
            .iter_mut()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| CommandError::not_found(format!("Agent not found: {}", agent_id)))?;
        agent.conversation.messages.clear();
        agent.agent_session_id = None;
        agent.session_memory.reset();
        agent.invalidate_cached_params();
        agent.is_dirty = true;
    }

    state
        .subagent_manager
        .cancel_all_for_parent(&agent_id)
        .await;

    let _ = crate::persistence::session::clear_agent_session(&agent_id);
    let _ = crate::runtime::agent_sdk::shutdown_bridge(&agent_id).await;

    let _ = app_handle.emit(
        "agent-conversation-cleared",
        serde_json::json!({
            "agent_id": agent_id,
            "checkpoint_id": checkpoint_id,
        }),
    );

    Ok(checkpoint_id)
}

/// Update an agent's resolved traits, regenerate COGNITIVE_PROFILE.md, and log the change.
/// Implements collaborative trait changes (D5): if the agent is active, injects a
/// notification so the agent is aware of the personality shift.
#[tauri::command]
pub async fn update_agent_traits(
    state: State<'_, Arc<AppState>>,
    app_handle: AppHandle,
    agent_id: String,
    traits: crate::config::agent_config::ResolvedTraits,
) -> Result<serde_json::Value, CommandError> {
    let dir = agent_dir(&agent_id);
    let config_path = dir.join("config.toml");

    if !config_path.exists() {
        return Err(CommandError::not_found(format!(
            "Agent not found: {}",
            agent_id
        )));
    }

    // Load and update config
    let mut config = AgentConfigFile::load(&config_path)
        .map_err(|e| CommandError::internal(format!("Failed to load config: {}", e)))?;

    let old_traits = config.agent.traits.clone();
    config.agent.traits = Some(traits.clone());
    config
        .save(&config_path)
        .map_err(|e| CommandError::internal(format!("Failed to save config: {}", e)))?;

    // Regenerate COGNITIVE_PROFILE.md from resolved traits
    let (primary_base, secondary_base_entry, spec) =
        load_registry_entries_for_traits(&traits, Some(&app_handle));
    let profile = crate::runtime::persona_registry::generate_cognitive_profile_from_traits(
        &traits,
        primary_base.as_ref(),
        secondary_base_entry.as_ref(),
        spec.as_ref(),
    );
    let persona_dir = dir.join("persona");
    let _ = std::fs::create_dir_all(&persona_dir);
    std::fs::write(persona_dir.join("COGNITIVE_PROFILE.md"), &profile)
        .map_err(|e| CommandError::internal(format!("Failed to write profile: {}", e)))?;

    // Build changelog summary showing what changed
    let summary = if let Some(ref old) = old_traits {
        let mut changes = Vec::new();
        if (old.openness - traits.openness).abs() > 0.01 {
            changes.push(format!("O: {:.2}->{:.2}", old.openness, traits.openness));
        }
        if (old.conscientiousness - traits.conscientiousness).abs() > 0.01 {
            changes.push(format!(
                "C: {:.2}->{:.2}",
                old.conscientiousness, traits.conscientiousness
            ));
        }
        if (old.extraversion - traits.extraversion).abs() > 0.01 {
            changes.push(format!(
                "E: {:.2}->{:.2}",
                old.extraversion, traits.extraversion
            ));
        }
        if (old.agreeableness - traits.agreeableness).abs() > 0.01 {
            changes.push(format!(
                "A: {:.2}->{:.2}",
                old.agreeableness, traits.agreeableness
            ));
        }
        if (old.neuroticism - traits.neuroticism).abs() > 0.01 {
            changes.push(format!(
                "N: {:.2}->{:.2}",
                old.neuroticism, traits.neuroticism
            ));
        }
        if old.verbosity != traits.verbosity {
            changes.push(format!(
                "verbosity: {}->{}",
                old.verbosity, traits.verbosity
            ));
        }
        if old.initiative != traits.initiative {
            changes.push(format!(
                "initiative: {}->{}",
                old.initiative, traits.initiative
            ));
        }
        if old.tone != traits.tone {
            changes.push(format!("tone: {}->{}", old.tone, traits.tone));
        }
        if changes.is_empty() {
            "Trait update (no significant changes)".to_string()
        } else {
            format!("Trait adjustment: {}", changes.join(", "))
        }
    } else {
        "Initial trait values set".to_string()
    };

    crate::runtime::persona::append_changelog(
        &agent_id,
        "COGNITIVE_PROFILE.md + config.toml [agent.traits]",
        &summary,
        &crate::runtime::persona::ChangeInitiator::Human,
    );

    // Collaborative trait change (D5): notify the agent if it's active
    let agent_online = {
        let agents = state.agents.read().await;
        agents.iter().any(|a| a.id == agent_id)
    };

    if agent_online {
        // Inject a user note so the agent sees the change on its next turn
        let note = format!(
            "[Personality Update] The human operator adjusted your traits. Changes: {}. \
            Your COGNITIVE_PROFILE.md has been regenerated. \
            Please read your updated profile and acknowledge the changes.",
            summary
        );
        let mut agents = state.agents.write().await;
        if let Some(agent) = agents.iter_mut().find(|a| a.id == agent_id) {
            agent.user_notes.push(note);
            agent.is_dirty = true;
        }
    }

    // Signal running agent to reload persona if active
    let _ = app_handle.emit(
        "persona-changed",
        serde_json::json!({ "agent_id": agent_id }),
    );

    Ok(serde_json::json!({
        "status": "ok",
        "agent_id": agent_id,
        "profile_chars": profile.len(),
        "summary": summary,
        "agent_notified": agent_online,
    }))
}

/// Preview a cognitive profile from traits without saving anything.
#[tauri::command]
pub async fn preview_cognitive_profile(
    app_handle: AppHandle,
    traits: crate::config::agent_config::ResolvedTraits,
) -> Result<String, CommandError> {
    let (primary_base, secondary_base_entry, spec) =
        load_registry_entries_for_traits(&traits, Some(&app_handle));
    Ok(
        crate::runtime::persona_registry::generate_cognitive_profile_from_traits(
            &traits,
            primary_base.as_ref(),
            secondary_base_entry.as_ref(),
            spec.as_ref(),
        ),
    )
}

/// Read the persona changelog for an agent (for the Personality tab changelog viewer).
#[tauri::command]
pub async fn get_persona_changelog(agent_id: String) -> Result<String, CommandError> {
    let dir = agent_dir(&agent_id);
    let changelog_path = dir.join("persona").join("PERSONA_CHANGELOG.md");
    if changelog_path.exists() {
        std::fs::read_to_string(&changelog_path)
            .map_err(|e| CommandError::internal(format!("Failed to read changelog: {}", e)))
    } else {
        Ok(String::new())
    }
}

/// Helper: load base, secondary base, and specialization from registry for a given ResolvedTraits.
fn load_registry_entries_for_traits(
    traits: &crate::config::agent_config::ResolvedTraits,
    app_handle: Option<&AppHandle>,
) -> (
    Option<crate::runtime::persona_registry::Base>,
    Option<crate::runtime::persona_registry::Base>,
    Option<crate::runtime::persona_registry::Specialization>,
) {
    let registry = crate::runtime::persona_registry::resolve_registry_dir(app_handle)
        .ok()
        .and_then(|dir| crate::runtime::persona_registry::load_registry(&dir).ok());

    let primary_base = registry.as_ref().and_then(|r| {
        traits
            .primary_base
            .as_deref()
            .and_then(|id| r.bases.get(id).cloned())
    });
    let secondary_base = registry.as_ref().and_then(|r| {
        traits
            .secondary_base
            .as_deref()
            .and_then(|id| r.bases.get(id).cloned())
    });
    let spec = registry.as_ref().and_then(|r| {
        traits
            .specialization
            .as_deref()
            .and_then(|id| r.specializations.get(id).cloned())
    });

    (primary_base, secondary_base, spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seed_identity_with_base_and_spec() {
        let result = generate_seed_identity_sentence(
            "Recon",
            Some("scout"),
            None,
            Some("code_reviewer"),
            Some("The explorer"),
        );
        assert!(result.contains("Your name is Recon."));
        assert!(result.contains("scout"));
        assert!(result.contains("code reviewer"));
        assert!(result.contains("the explorer"));
    }

    #[test]
    fn test_seed_identity_with_base_only() {
        let result = generate_seed_identity_sentence(
            "Warden",
            Some("sentinel"),
            None,
            None,
            Some("The watchdog"),
        );
        assert!(result.contains("Your name is Warden."));
        assert!(result.contains("sentinel"));
        assert!(result.contains("the watchdog"));
        assert!(!result.contains("specializing"));
    }

    #[test]
    fn test_seed_identity_with_base_no_tagline() {
        let result = generate_seed_identity_sentence(
            "Nova",
            Some("catalyst"),
            None,
            Some("red_teamer"),
            None,
        );
        assert!(result.contains("Your name is Nova."));
        assert!(result.contains("catalyst"));
    }

    #[test]
    fn test_seed_identity_custom() {
        let result = generate_seed_identity_sentence("Ghost", None, None, None, None);
        assert!(result.contains("Your name is Ghost."));
        assert!(result.contains("hand-crafted"));
    }

    #[test]
    fn test_seed_identity_underscore_replacement() {
        let result = generate_seed_identity_sentence(
            "Test",
            Some("red_teamer"),
            None,
            Some("competitive_intelligence"),
            Some("The attacker."),
        );
        assert!(result.contains("red teamer"));
        assert!(result.contains("competitive intelligence"));
        // Tagline period should be trimmed
        assert!(!result.contains("the attacker.."));
    }

    #[test]
    fn test_seed_identity_with_blended_bases() {
        let result = generate_seed_identity_sentence(
            "Avery",
            Some("architect"),
            Some("catalyst"),
            Some("creative_ideas"),
            Some("The strategist"),
        );
        assert!(result.contains("Your name is Avery."));
        assert!(result.contains("architect blended with catalyst"));
        assert!(result.contains("creative ideas"));
    }

    #[test]
    fn test_seed_constants_not_empty() {
        assert!(!SEED_HOLT_ORIENTATION.is_empty());
        assert!(!SEED_ETHOS.is_empty());
        assert!(SEED_HOLT_ORIENTATION.contains("Holt"));
        assert!(SEED_HOLT_ORIENTATION.contains("Hillock"));
        assert!(SEED_ETHOS.contains("honest"));
        assert!(SEED_ETHOS.contains("performative"));
    }
}
