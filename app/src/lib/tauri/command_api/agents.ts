import { invoke } from './shared';
import type { AgentColor, AgentProtocol, AgentStatus } from './shared';

export interface AgentSummary {
  id: string;
  name: string;
  color: AgentColor;
  status: AgentStatus;
  protocol: AgentProtocol;
  working_directory: string;
  message_count: number;
}

export interface DetectedModel {
  service_name: string;
  endpoint: string;
  port: number;
  models: string[];
}

export interface ContentBlock {
  type: 'text' | 'image';
  text?: string;
  data?: string;
  media_type?: string;
}

export interface ConversationMessageSummary {
  role: string;
  content: string;
  timestamp: string;
  tool_calls: { id: string; name: string; arguments: unknown }[] | null;
  tool_call_id: string | null;
  content_blocks?: ContentBlock[];
}

export interface CreateAgentParams {
  name: string;
  connectionType: string;
  endpoint?: string;
  model?: string;
  host?: string;
  port?: number;
  workingDirectory: string;
  colorR?: number;
  colorG?: number;
  colorB?: number;
  keyRef?: string;
  personaTemplate?: string;
  agentSessionId?: string;
  archetypeBase?: string;
  archetypeSecondaryBase?: string;
  archetypeBlendWeight?: number;
  archetypeSpecialization?: string;
  commVerbosity?: string;
  commInitiative?: string;
  commTone?: string;
  oceanOverrides?: Record<string, number>;
  toolsFilesystem?: boolean;
  toolsCodeExecution?: boolean;
  toolsWebAccess?: boolean;
  toolsVerification?: boolean;
  toolsSubagent?: boolean;
  toolsA2aMessaging?: boolean;
}

export interface OceanScores {
  openness: number;
  conscientiousness: number;
  extraversion: number;
  agreeableness: number;
  neuroticism: number;
}

export interface ArchetypeBaseSummary {
  id: string;
  name: string;
  tagline: string;
  description: string;
  ocean: OceanScores;
  belbin_primary: string;
  disc_style: string;
  cognition_style: string;
}

export interface ArchetypeSpecSummary {
  id: string;
  name: string;
  department: string;
  tagline: string;
  description: string;
  ocean_modifiers: OceanScores;
}

export interface ArchetypeRegistryListing {
  bases: ArchetypeBaseSummary[];
  specializations: ArchetypeSpecSummary[];
}

export interface PersonaTemplateSummary {
  id: string;
  name: string;
  description: string;
}

export interface ImageAttachment {
  data: string;
  media_type: string;
  filename?: string;
}

export interface SubagentInfo {
  job_id: string;
  name: string;
  task: string;
  status: string | { Failed: string };
  elapsed_ms: number;
  created_at: string;
}

export interface FullAgentConfig {
  id: string;
  name: string;
  color: AgentColor;
  connection_type: string;
  endpoint: string;
  model: string;
  host: string;
  port: number;
  key_ref: string;
  working_directory: string;
  system_prompt: string;
  tools_filesystem: boolean;
  tools_code_execution: boolean;
  tools_web_access: boolean;
  tools_verification: boolean;
  tools_subagent: boolean;
  tools_a2a_messaging: boolean;
  sandbox_enabled: boolean;
  max_sandbox_iterations: number;
  max_tool_calls_per_turn: number;
  max_tool_calls_per_session: number;
  response_timeout_seconds: number;
  max_response_tokens: number;
  autonomous: boolean;
  agent_session_id: string;
  traits: ResolvedTraits | null;
}

export interface UpdateAgentParams {
  agentId: string;
  name?: string;
  colorR?: number;
  colorG?: number;
  colorB?: number;
  workingDirectory?: string;
  systemPrompt?: string;
  connectionType?: string;
  endpoint?: string;
  model?: string;
  host?: string;
  port?: number;
  toolsFilesystem?: boolean;
  toolsCodeExecution?: boolean;
  toolsWebAccess?: boolean;
  toolsVerification?: boolean;
  toolsSubagent?: boolean;
  toolsA2aMessaging?: boolean;
  keyRef?: string;
  maxToolCallsPerTurn?: number;
  maxToolCallsPerSession?: number;
  responseTimeoutSeconds?: number;
  maxResponseTokens?: number;
  sandboxEnabled?: boolean;
  maxSandboxIterations?: number;
  autonomous?: boolean;
  agentSessionId?: string;
}

export interface ContextStatus {
  context_tokens: number;
  context_window_size: number;
  message_count: number;
  needs_compaction: boolean;
}

export function listAgents(): Promise<AgentSummary[]> {
  return invoke<AgentSummary[]>('list_agents');
}

export function createAgent(params: CreateAgentParams): Promise<string> {
  return invoke<string>('create_agent', { ...params });
}

export function listArchetypes(): Promise<ArchetypeRegistryListing> {
  return invoke<ArchetypeRegistryListing>('list_archetypes');
}

export function listPersonaTemplates(): Promise<PersonaTemplateSummary[]> {
  return invoke<PersonaTemplateSummary[]>('list_persona_templates');
}

export function removeAgent(agentId: string): Promise<void> {
  return invoke<void>('remove_agent', { agentId });
}

export function storeApiKey(agentId: string, apiKey: string): Promise<void> {
  return invoke<void>('store_api_key', { agentId, apiKey });
}

export function scanLocalModels(): Promise<DetectedModel[]> {
  return invoke<DetectedModel[]>('scan_local_models');
}

export function getConversation(agentId: string): Promise<ConversationMessageSummary[]> {
  return invoke<ConversationMessageSummary[]>('get_conversation', { agentId });
}

export function sendMessage(agentId: string, message: string, images?: ImageAttachment[]): Promise<string> {
  return invoke<string>('send_message', { agentId, message, images: images ?? null });
}

export function runCodexReview(agentId: string, instructions?: string): Promise<string> {
  return invoke<string>('run_codex_review', { agentId, instructions: instructions ?? null });
}

export function triggerAgentTurn(agentId: string): Promise<void> {
  return invoke<void>('trigger_agent_turn', { agentId });
}

export function checkVisionSupport(agentId: string): Promise<boolean> {
  return invoke<boolean>('check_vision_support', { agentId });
}

export function respondToHil(agentId: string, response: string, selectedOption?: number): Promise<void> {
  return invoke<void>('respond_to_hil', {
    agentId,
    response,
    selectedOption: selectedOption ?? null,
  });
}

export function respondToApproval(agentId: string, toolCallId: string, approved: boolean): Promise<void> {
  return invoke<void>('respond_to_approval', {
    agentId,
    toolCallId,
    approved,
  });
}

export function interruptAgent(agentId: string): Promise<void> {
  return invoke<void>('interrupt_agent', { agentId });
}

export function tagMessageOutcome(agentId: string, messageIndex: number, outcome: string | null): Promise<void> {
  return invoke<void>('tag_message_outcome', {
    agentId,
    messageIndex,
    outcome,
  });
}

export function listSubagents(agentId: string): Promise<SubagentInfo[]> {
  return invoke<SubagentInfo[]>('list_subagents', { agentId });
}

export function cancelSubagent(jobId: string): Promise<void> {
  return invoke<void>('cancel_subagent', { jobId });
}

export function getAgentDetails(agentId: string): Promise<FullAgentConfig> {
  return invoke<FullAgentConfig>('get_agent_details', { agentId });
}

export function updateAgent(params: UpdateAgentParams): Promise<void> {
  return invoke<void>('update_agent', { ...params });
}

export function clearConversation(agentId: string): Promise<string> {
  return invoke<string>('clear_conversation', { agentId });
}

export function getContextStatus(agentId: string): Promise<ContextStatus> {
  return invoke<ContextStatus>('get_context_status', { agentId });
}

export function compactAgentContext(agentId: string): Promise<string> {
  return invoke<string>('compact_agent_context', { agentId });
}

export function broadcastMessage(message: string): Promise<string[]> {
  return invoke<string[]>('broadcast_message', { message });
}

// --- Codex thread operations ---

export function codexSwitchModel(agentId: string, model: string): Promise<string> {
  return invoke<string>('codex_switch_model', { agentId, model });
}

export function codexSetEffort(agentId: string, level: string): Promise<string> {
  return invoke<string>('codex_set_effort', { agentId, level });
}

export function codexSetApproval(agentId: string, policy: string): Promise<string> {
  return invoke<string>('codex_set_approval', { agentId, policy });
}

export function codexSetGoal(agentId: string, goalText?: string): Promise<string> {
  return invoke<string>('codex_set_goal', { agentId, goalText: goalText ?? null });
}

export function codexForkThread(agentId: string): Promise<string> {
  return invoke<string>('codex_fork_thread', { agentId });
}

export function codexRollback(agentId: string, numTurns: number): Promise<string> {
  return invoke<string>('codex_rollback', { agentId, numTurns });
}

export function codexShellCommand(agentId: string, command: string): Promise<string> {
  return invoke<string>('codex_shell_command', { agentId, command });
}

export function addUserNote(agentId: string, note: string): Promise<number> {
  return invoke<number>('add_user_note', { agentId, note });
}

export function clearUserNotes(agentId: string): Promise<void> {
  return invoke('clear_user_notes', { agentId });
}

// ── Resolved Traits (P2/P3: archetype blending) ─────────────────────────

export interface ResolvedTraits {
  // Provenance
  primary_base?: string;
  secondary_base?: string;
  blend_weight?: number;
  specialization?: string;

  // OCEAN (authoritative)
  openness: number;
  conscientiousness: number;
  extraversion: number;
  agreeableness: number;
  neuroticism: number;

  // Communication
  verbosity: string;
  initiative: string;
  tone: string;

  // Manual overrides
  manual_overrides: string[];

  // Custom onboarding
  directives_pending: boolean;
}

export interface UpdateTraitsResult {
  status: string;
  agent_id: string;
  profile_chars: number;
  summary: string;
}

export function updateAgentTraits(agentId: string, traits: ResolvedTraits): Promise<UpdateTraitsResult> {
  return invoke<UpdateTraitsResult>('update_agent_traits', { agentId, traits });
}

export function previewCognitiveProfile(traits: ResolvedTraits): Promise<string> {
  return invoke<string>('preview_cognitive_profile', { traits });
}

export function getPersonaChangelog(agentId: string): Promise<string> {
  return invoke<string>('get_persona_changelog', { agentId });
}
