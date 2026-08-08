<script lang="ts">
  import {
    getAgentDetails,
    updateAgent,
    updateAgentTraits,
    getPersonaChangelog,
    testConnection,
    pickFolder,
    extractErrorMessage,
    type FullAgentConfig,
    type UpdateAgentParams,
    type ResolvedTraits,
    type OceanScores,
  } from '$lib/tauri/commands';
  import { DEBOUNCE_MS, AGENT_COLOR_PRESETS } from '$lib/constants';
  import OceanRadar from '$lib/ui/OceanRadar.svelte';

  interface Props {
    agentId: string | null;
  }

  let { agentId }: Props = $props();

  // Agent config state
  let agent = $state<FullAgentConfig | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  // Connection test state
  let testResult = $state<'idle' | 'testing' | 'pass' | 'fail'>('idle');

  // Section collapse state — all open by default
  let sections = $state({
    identity: true,
    personality: false,
    connection: true,
    systemPrompt: true,
    tools: true,
    limits: true,
    workspace: true,
    approval: true,
  });

  // Personality section state
  let editingTraits = $state<ResolvedTraits | null>(null);
  let changelog = $state('');
  let traitsDirty = $state(false);
  let traitsSaving = $state(false);
  let magnitudeWarning = $state(false);
  let magnitudeConfirmed = $state(false);

  let editingOcean = $derived<OceanScores>(editingTraits ? {
    openness: editingTraits.openness,
    conscientiousness: editingTraits.conscientiousness,
    extraversion: editingTraits.extraversion,
    agreeableness: editingTraits.agreeableness,
    neuroticism: editingTraits.neuroticism,
  } : { openness: 0.5, conscientiousness: 0.5, extraversion: 0.5, agreeableness: 0.5, neuroticism: 0.5 });

  function initTraitsEditor() {
    if (agent?.traits) {
      editingTraits = { ...agent.traits };
    } else {
      editingTraits = {
        openness: 0.5, conscientiousness: 0.5, extraversion: 0.5,
        agreeableness: 0.5, neuroticism: 0.5,
        verbosity: 'balanced', initiative: 'measured', tone: 'balanced',
        manual_overrides: [], directives_pending: false,
      };
    }
    traitsDirty = false;
    magnitudeWarning = false;
  }

  function handleTraitRadarChange(trait: keyof OceanScores, value: number) {
    if (!editingTraits) return;
    editingTraits[trait] = value;
    editingTraits = { ...editingTraits }; // trigger reactivity
    checkMagnitude();
    magnitudeConfirmed = false;
    traitsDirty = true;
  }

  function checkMagnitude() {
    if (!editingTraits || !agent?.traits) { magnitudeWarning = false; return; }
    const orig = agent.traits;
    const keys: (keyof OceanScores)[] = ['openness', 'conscientiousness', 'extraversion', 'agreeableness', 'neuroticism'];
    magnitudeWarning = keys.some(k => Math.abs(editingTraits![k] - orig[k]) > 0.3);
  }

  async function saveTraits() {
    if (!editingTraits || !agentId) return;
    // Magnitude friction: require explicit confirmation for large shifts
    if (magnitudeWarning && !magnitudeConfirmed) {
      magnitudeConfirmed = true;
      return; // User must click save again to confirm
    }
    traitsSaving = true;
    try {
      await updateAgentTraits(agentId, editingTraits);
      // Reload agent to get fresh state
      await loadAgent(agentId);
      initTraitsEditor();
      // Refresh changelog
      changelog = await getPersonaChangelog(agentId);
    } catch (e) {
      console.error('Failed to save traits:', e);
    } finally {
      traitsSaving = false;
    }
  }

  function resetTraits() {
    initTraitsEditor();
  }

  // Preset color swatches
  // Tool toggle definitions
  const TOOL_TOGGLES: { label: string; key: keyof FullAgentConfig; param: string }[] = [
    { label: 'Code Execution', key: 'tools_code_execution', param: 'toolsCodeExecution' },
    { label: 'Filesystem', key: 'tools_filesystem', param: 'toolsFilesystem' },
    { label: 'Web Search', key: 'tools_web_access', param: 'toolsWebAccess' },
    { label: 'Verification', key: 'tools_verification', param: 'toolsVerification' },
    { label: 'Subagent Spawning', key: 'tools_subagent', param: 'toolsSubagent' },
    { label: 'A2A Messaging', key: 'tools_a2a_messaging', param: 'toolsA2aMessaging' },
  ];

  const COLOR_PRESETS = AGENT_COLOR_PRESETS;

  type AutosaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error';

  // Debounced autosave
  let saveTimeout: ReturnType<typeof setTimeout> | null = null;
  let saveStatusTimeout: ReturnType<typeof setTimeout> | null = null;
  let saveGeneration = 0;
  let pendingAutosaveChanges: Partial<UpdateAgentParams> = {};
  let autosaveStatus = $state<AutosaveStatus>('idle');
  let autosaveError = $state<string | null>(null);

  function clearAutosaveTimers() {
    if (saveTimeout) {
      clearTimeout(saveTimeout);
      saveTimeout = null;
    }
    if (saveStatusTimeout) {
      clearTimeout(saveStatusTimeout);
      saveStatusTimeout = null;
    }
  }

  function resetAutosaveState() {
    saveGeneration += 1;
    pendingAutosaveChanges = {};
    clearAutosaveTimers();
    autosaveStatus = 'idle';
    autosaveError = null;
  }

  function scheduleAutosave(changes: Partial<UpdateAgentParams>) {
    if (!agentId) return;
    const targetAgentId = agentId;
    const generation = ++saveGeneration;

    pendingAutosaveChanges = { ...pendingAutosaveChanges, ...changes };
    autosaveStatus = 'pending';
    autosaveError = null;

    if (saveTimeout) {
      clearTimeout(saveTimeout);
    }
    if (saveStatusTimeout) {
      clearTimeout(saveStatusTimeout);
      saveStatusTimeout = null;
    }

    saveTimeout = setTimeout(async () => {
      const payload = { ...pendingAutosaveChanges };
      saveTimeout = null;
      if (Object.keys(payload).length === 0) {
        autosaveStatus = 'idle';
        return;
      }
      autosaveStatus = 'saving';

      try {
        await updateAgent({ agentId: targetAgentId, ...payload });
        if (generation !== saveGeneration || agentId !== targetAgentId) return;

        pendingAutosaveChanges = {};
        autosaveStatus = 'saved';
        saveStatusTimeout = setTimeout(() => {
          if (generation === saveGeneration && agentId === targetAgentId) {
            autosaveStatus = 'idle';
          }
        }, 1800);
      } catch (e) {
        if (generation !== saveGeneration || agentId !== targetAgentId) return;

        autosaveStatus = 'error';
        autosaveError = extractErrorMessage(e);
        console.error('Failed to autosave agent settings:', e);
      }
    }, DEBOUNCE_MS);
  }

  // Load agent details
  async function loadAgent(id: string) {
    loading = true;
    error = null;
    testResult = 'idle';
    resetAutosaveState();
    try {
      agent = await getAgentDetails(id);
      initTraitsEditor();
      // Load changelog in background
      getPersonaChangelog(id).then(c => { changelog = c; }).catch(() => { changelog = ''; });
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      agent = null;
    } finally {
      loading = false;
    }
  }

  // React to agentId changes
  $effect(() => {
    if (agentId) {
      loadAgent(agentId);
    } else {
      agent = null;
      error = null;
      resetAutosaveState();
    }
    return () => {
      resetAutosaveState();
    };
  });

  function toggleSection(key: keyof typeof sections) {
    sections[key] = !sections[key];
  }

  function rgbToHex(r: number, g: number, b: number): string {
    return `#${((1 << 24) + (r << 16) + (g << 8) + b).toString(16).slice(1)}`;
  }

  function colorsMatch(a: { r: number; g: number; b: number }, b: { r: number; g: number; b: number }): boolean {
    return a.r === b.r && a.g === b.g && a.b === b.b;
  }

  function selectColor(preset: { r: number; g: number; b: number }) {
    if (!agent) return;
    agent.color = { ...preset };
    scheduleAutosave({ colorR: preset.r, colorG: preset.g, colorB: preset.b });
  }

  async function handleTestConnection() {
    if (!agent) return;
    testResult = 'testing';
    try {
      const ok = await testConnection(agent.key_ref);
      testResult = ok ? 'pass' : 'fail';
    } catch {
      testResult = 'fail';
    }
  }

  async function handlePickFolder() {
    if (!agent) return;
    const folder = await pickFolder(agent.working_directory || undefined);
    if (folder) {
      agent.working_directory = folder;
      scheduleAutosave({ workingDirectory: folder });
    }
  }
</script>

{#if !agentId}
  <div class="empty-state">Select an agent to configure</div>
{:else if loading}
  <div class="empty-state">Loading...</div>
{:else if error}
  <div class="empty-state error">{error}</div>
{:else if agent}
  <div class="agent-sections">
    {#if autosaveStatus !== 'idle'}
      <div
        class="autosave-status"
        class:pending={autosaveStatus === 'pending'}
        class:saving={autosaveStatus === 'saving'}
        class:saved={autosaveStatus === 'saved'}
        class:error={autosaveStatus === 'error'}
        role={autosaveStatus === 'error' ? 'alert' : 'status'}
        aria-live="polite"
      >
        {#if autosaveStatus === 'pending'}
          Unsaved changes…
        {:else if autosaveStatus === 'saving'}
          Saving settings…
        {:else if autosaveStatus === 'saved'}
          Settings saved
        {:else if autosaveStatus === 'error'}
          Save failed: {autosaveError ?? 'Unknown error'}
        {/if}
      </div>
    {/if}

    <!-- 1. Identity -->
    <div class="config-section">
      <button class="section-header" onclick={() => toggleSection('identity')}>
        <div class="dot" style="background: #06b6d4;"></div>
        <span class="section-label">Identity</span>
        <span class="chevron" class:collapsed={!sections.identity}></span>
      </button>
      {#if sections.identity}
        <div class="section-body">
          <div class="config-field">
            <label for="agent-name">Name</label>
            <input
              id="agent-name"
              type="text"
              bind:value={agent.name}
              oninput={() => scheduleAutosave({ name: agent!.name })}
            />
          </div>
          <div class="config-field">
            <span class="field-label">Color</span>
            <div class="color-swatches">
              {#each COLOR_PRESETS as preset}
                <button
                  class="swatch"
                  class:selected={colorsMatch(agent.color, preset)}
                  style="background: {rgbToHex(preset.r, preset.g, preset.b)};"
                  onclick={() => selectColor(preset)}
                  aria-label="Select color {rgbToHex(preset.r, preset.g, preset.b)}"
                ></button>
              {/each}
            </div>
          </div>
        </div>
      {/if}
    </div>

    <!-- 2. Personality -->
    <div class="config-section">
      <button class="section-header" onclick={() => toggleSection('personality')}>
        <div class="dot" style="background: #f97316;"></div>
        <span class="section-label">Personality</span>
        <span class="chevron" class:collapsed={!sections.personality}></span>
      </button>
      {#if sections.personality}
        <div class="section-body">
          {#if editingTraits}
            <!-- Provenance info -->
            {#if editingTraits.primary_base}
              <div class="provenance">
                <span class="provenance-label">Base:</span>
                <span class="provenance-value">{editingTraits.primary_base.replace(/_/g, ' ')}</span>
                {#if editingTraits.secondary_base}
                  <span class="provenance-blend">+ {editingTraits.secondary_base.replace(/_/g, ' ')} ({((editingTraits.blend_weight ?? 1) * 100).toFixed(0)}%)</span>
                {/if}
                {#if editingTraits.specialization}
                  <span class="provenance-spec">/ {editingTraits.specialization.replace(/_/g, ' ')}</span>
                {/if}
              </div>
            {:else}
              <div class="provenance">
                <span class="provenance-label">Base:</span>
                <span class="provenance-value">Custom</span>
              </div>
            {/if}

            <!-- Interactive radar -->
            <div class="radar-container">
              <OceanRadar
                scores={editingOcean}
                size={180}
                interactive={true}
                onchange={handleTraitRadarChange}
              />
            </div>

            <!-- OCEAN value readout -->
            <div class="ocean-readout">
              {#each [
                { key: 'openness', label: 'O' },
                { key: 'conscientiousness', label: 'C' },
                { key: 'extraversion', label: 'E' },
                { key: 'agreeableness', label: 'A' },
                { key: 'neuroticism', label: 'N' },
              ] as trait}
                <div class="ocean-value">
                  <span class="ocean-label">{trait.label}</span>
                  <span class="ocean-num">{(editingTraits[trait.key as keyof OceanScores] * 100).toFixed(0)}</span>
                </div>
              {/each}
            </div>

            <!-- Communication controls -->
            <div class="comm-controls">
              <!-- svelte-ignore a11y_label_has_associated_control -->
              <label class="comm-item">
                <span>Verbosity</span>
                <select
                  bind:value={editingTraits.verbosity}
                  onchange={() => { traitsDirty = true; }}
                >
                  <option value="terse">Terse</option>
                  <option value="concise">Concise</option>
                  <option value="balanced">Balanced</option>
                  <option value="thorough">Thorough</option>
                  <option value="verbose">Verbose</option>
                </select>
              </label>
              <!-- svelte-ignore a11y_label_has_associated_control -->
              <label class="comm-item">
                <span>Initiative</span>
                <select
                  bind:value={editingTraits.initiative}
                  onchange={() => { traitsDirty = true; }}
                >
                  <option value="reactive">Reactive</option>
                  <option value="measured">Measured</option>
                  <option value="proactive">Proactive</option>
                </select>
              </label>
              <!-- svelte-ignore a11y_label_has_associated_control -->
              <label class="comm-item">
                <span>Tone</span>
                <select
                  bind:value={editingTraits.tone}
                  onchange={() => { traitsDirty = true; }}
                >
                  <option value="direct">Direct</option>
                  <option value="balanced">Balanced</option>
                  <option value="warm">Warm</option>
                </select>
              </label>
            </div>

            <!-- Magnitude warning -->
            {#if magnitudeWarning}
              <div class="magnitude-warning">
                {#if magnitudeConfirmed}
                  Click Save again to confirm the large personality shift.
                {:else}
                  Significant personality shift detected (&gt;30% on one or more traits). Consider making incremental changes.
                {/if}
              </div>
            {/if}

            <!-- Save / Reset buttons -->
            {#if traitsDirty}
              <div class="traits-actions">
                <button class="btn-save" class:confirming={magnitudeWarning && magnitudeConfirmed} onclick={saveTraits} disabled={traitsSaving}>
                  {traitsSaving ? 'Saving...' : magnitudeConfirmed ? 'Confirm Save' : 'Save Changes'}
                </button>
                <button class="btn-reset" onclick={resetTraits}>Reset</button>
              </div>
            {/if}

            <!-- Changelog viewer -->
            {#if changelog}
              <div class="changelog-section">
                <span class="changelog-header">Changelog</span>
                <div class="changelog-viewer">
                  <pre>{changelog}</pre>
                </div>
              </div>
            {/if}
          {:else}
            <p class="approval-note">No personality traits configured for this agent.</p>
          {/if}
        </div>
      {/if}
    </div>

    <!-- 3. Connection -->
    <div class="config-section">
      <button class="section-header" onclick={() => toggleSection('connection')}>
        <div class="dot" style="background: #6366f1;"></div>
        <span class="section-label">Connection</span>
        <span class="chevron" class:collapsed={!sections.connection}></span>
      </button>
      {#if sections.connection}
        <div class="section-body">
          <div class="config-field">
            <span class="field-label">Type</span>
            <div class="type-buttons">
              <button
                class="type-btn"
                class:active={agent.connection_type === 'api'}
                onclick={() => { agent!.connection_type = 'api'; scheduleAutosave({ connectionType: 'api' }); }}
              >API</button>
              <button
                class="type-btn"
                class:active={agent.connection_type === 'local'}
                onclick={() => { agent!.connection_type = 'local'; scheduleAutosave({ connectionType: 'local' }); }}
              >Local</button>
              <button
                class="type-btn"
                class:active={agent.connection_type === 'mcp_stdio'}
                onclick={() => { agent!.connection_type = 'mcp_stdio'; scheduleAutosave({ connectionType: 'mcp_stdio' }); }}
              >MCP</button>
              <button
                class="type-btn"
                class:active={agent.connection_type === 'codex'}
                onclick={() => { agent!.connection_type = 'codex'; scheduleAutosave({ connectionType: 'codex' }); }}
              >Codex</button>
              <button
                class="type-btn"
                class:active={agent.connection_type === 'claude_code'}
                onclick={() => { agent!.connection_type = 'claude_code'; scheduleAutosave({ connectionType: 'claude_code' }); }}
              >Claude Code</button>
            </div>
          </div>
          {#if agent.connection_type !== 'mcp_stdio' && agent.connection_type !== 'claude_code'}
            <div class="config-field">
              <label for="agent-model">Model</label>
              <input
                id="agent-model"
                type="text"
                bind:value={agent.model}
                oninput={() => scheduleAutosave({ model: agent!.model })}
              />
            </div>
          {/if}
          {#if agent.connection_type === 'codex'}
            <div class="config-field">
              <label for="agent-codex-session-id">Session ID</label>
              <input
                id="agent-codex-session-id"
                type="text"
                class="mono"
                bind:value={agent.agent_session_id}
                oninput={() => scheduleAutosave({ agentSessionId: agent!.agent_session_id })}
                placeholder="Leave blank to start fresh"
              />
            </div>
          {/if}
          {#if agent.connection_type === 'api'}
            <div class="config-field">
              <label for="agent-endpoint">Endpoint</label>
              <input
                id="agent-endpoint"
                type="text"
                bind:value={agent.endpoint}
                oninput={() => scheduleAutosave({ endpoint: agent!.endpoint })}
              />
            </div>
          {/if}
          {#if agent.connection_type === 'local'}
            <div class="field-row">
              <div class="config-field" style="flex: 1;">
                <label for="agent-host">Host</label>
                <input
                  id="agent-host"
                  type="text"
                  bind:value={agent.host}
                  oninput={() => scheduleAutosave({ host: agent!.host })}
                />
              </div>
              <div class="config-field" style="width: 80px;">
                <label for="agent-port">Port</label>
                <input
                  id="agent-port"
                  type="number"
                  bind:value={agent.port}
                  oninput={() => scheduleAutosave({ port: agent!.port })}
                />
              </div>
            </div>
          {/if}
          {#if agent.connection_type === 'api'}
            <div class="config-field">
              <label for="agent-keyref">API Key Reference</label>
              <input
                id="agent-keyref"
                type="text"
                bind:value={agent.key_ref}
                oninput={() => scheduleAutosave({ keyRef: agent!.key_ref })}
              />
            </div>
            <div class="test-row">
              <button class="btn-test" onclick={handleTestConnection} disabled={testResult === 'testing'}>
                {testResult === 'testing' ? 'Testing...' : 'Test Connection'}
              </button>
              {#if testResult === 'pass'}
                <span class="test-status pass">Connected</span>
              {:else if testResult === 'fail'}
                <span class="test-status fail">Failed</span>
              {/if}
            </div>
          {/if}
        </div>
      {/if}
    </div>

    <!-- 4. System Prompt -->
    <div class="config-section">
      <button class="section-header" onclick={() => toggleSection('systemPrompt')}>
        <div class="dot" style="background: #f59e0b;"></div>
        <span class="section-label">System Prompt</span>
        <span class="chevron" class:collapsed={!sections.systemPrompt}></span>
      </button>
      {#if sections.systemPrompt}
        <div class="section-body">
          <div class="config-field">
            <textarea
              class="system-prompt"
              bind:value={agent.system_prompt}
              oninput={() => scheduleAutosave({ systemPrompt: agent!.system_prompt })}
            ></textarea>
          </div>
        </div>
      {/if}
    </div>

    <!-- 5. Tools -->
    <div class="config-section">
      <button class="section-header" onclick={() => toggleSection('tools')}>
        <div class="dot" style="background: #22c55e;"></div>
        <span class="section-label">Tools</span>
        <span class="chevron" class:collapsed={!sections.tools}></span>
      </button>
      {#if sections.tools}
        <div class="section-body">
          {#each TOOL_TOGGLES as toggle}
            <div class="toggle-row">
              <span>{toggle.label}</span>
              <button
                class="toggle"
                class:on={agent[toggle.key] as boolean}
                onclick={() => {
                  const current = agent![toggle.key] as boolean;
                  (agent as any)[toggle.key] = !current;
                  scheduleAutosave({ [toggle.param]: !current } as any);
                }}
                aria-label="Toggle {toggle.label}"
              ></button>
            </div>
          {/each}
        </div>
      {/if}
    </div>

    <!-- 6. Limits -->
    <div class="config-section">
      <button class="section-header" onclick={() => toggleSection('limits')}>
        <div class="dot" style="background: #a855f7;"></div>
        <span class="section-label">Limits</span>
        <span class="chevron" class:collapsed={!sections.limits}></span>
      </button>
      {#if sections.limits}
        <div class="section-body">
          <div class="limits-grid">
            <div class="config-field">
              <label for="limit-turn">Tools/Turn</label>
              <input
                id="limit-turn"
                type="number"
                bind:value={agent.max_tool_calls_per_turn}
                oninput={() => scheduleAutosave({ maxToolCallsPerTurn: agent!.max_tool_calls_per_turn })}
              />
            </div>
            <div class="config-field">
              <label for="limit-session">Tools/Session</label>
              <input
                id="limit-session"
                type="number"
                bind:value={agent.max_tool_calls_per_session}
                oninput={() => scheduleAutosave({ maxToolCallsPerSession: agent!.max_tool_calls_per_session })}
              />
            </div>
            <div class="config-field">
              <label for="limit-timeout">Timeout (s)</label>
              <input
                id="limit-timeout"
                type="number"
                bind:value={agent.response_timeout_seconds}
                oninput={() => scheduleAutosave({ responseTimeoutSeconds: agent!.response_timeout_seconds })}
              />
            </div>
            <div class="config-field">
              <label for="limit-tokens">Max Tokens</label>
              <input
                id="limit-tokens"
                type="number"
                bind:value={agent.max_response_tokens}
                oninput={() => scheduleAutosave({ maxResponseTokens: agent!.max_response_tokens })}
              />
            </div>
          </div>
        </div>
      {/if}
    </div>

    <!-- 7. Workspace -->
    <div class="config-section">
      <button class="section-header" onclick={() => toggleSection('workspace')}>
        <div class="dot" style="background: #06b6d4;"></div>
        <span class="section-label">Workspace</span>
        <span class="chevron" class:collapsed={!sections.workspace}></span>
      </button>
      {#if sections.workspace}
        <div class="section-body">
          <div class="config-field">
            <label for="agent-workdir">Working Directory</label>
            <div class="field-row">
              <input
                id="agent-workdir"
                type="text"
                bind:value={agent.working_directory}
                oninput={() => scheduleAutosave({ workingDirectory: agent!.working_directory })}
                style="flex: 1;"
              />
              <button class="btn-test btn-folder" onclick={handlePickFolder} aria-label="Browse folder">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
                </svg>
              </button>
            </div>
          </div>
          <div class="toggle-row">
            <span>Sandbox Enabled</span>
            <button
              class="toggle"
              class:on={agent.sandbox_enabled}
              onclick={() => {
                agent!.sandbox_enabled = !agent!.sandbox_enabled;
                scheduleAutosave({ sandboxEnabled: agent!.sandbox_enabled });
              }}
              aria-label="Toggle sandbox"
            ></button>
          </div>
          <div class="config-field">
            <label for="sandbox-iters">Max Iterations</label>
            <input
              id="sandbox-iters"
              type="number"
              bind:value={agent.max_sandbox_iterations}
              oninput={() => scheduleAutosave({ maxSandboxIterations: agent!.max_sandbox_iterations })}
              style="width: 80px;"
            />
          </div>
          <div class="toggle-row">
            <span>Autonomous Mode</span>
            <button
              class="toggle"
              class:on={agent.autonomous}
              onclick={() => {
                agent!.autonomous = !agent!.autonomous;
                scheduleAutosave({ autonomous: agent!.autonomous });
              }}
              aria-label="Toggle autonomous mode"
            ></button>
          </div>
        </div>
      {/if}
    </div>

    <!-- 8. Approval Policy -->
    <div class="config-section">
      <button class="section-header" onclick={() => toggleSection('approval')}>
        <div class="dot" style="background: #ec4899;"></div>
        <span class="section-label">Approval Policy</span>
        <span class="chevron" class:collapsed={!sections.approval}></span>
      </button>
      {#if sections.approval}
        <div class="section-body">
          <p class="approval-note">
            Approval tiers are configured via the agent TOML file.
            Four tiers available: Auto, NotifyUnlessVeto, RequireApproval, Blocked.
          </p>
        </div>
      {/if}
    </div>

  </div>
{/if}

<style>
  .empty-state {
    color: var(--text-muted, #64748b);
    font-size: 0.857rem;
    text-align: center;
    padding: 40px 0;
  }

  .empty-state.error {
    color: var(--alert-red, #ef4444);
  }

  .agent-sections {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .autosave-status {
    border: 1px solid var(--surface-border, #334155);
    border-radius: 8px;
    padding: 6px 8px;
    margin-bottom: 10px;
    font-size: 0.786rem;
    line-height: 1.4;
    color: var(--text-secondary, #94a3b8);
    background: color-mix(in srgb, var(--card-bg, #1e293b) 82%, transparent);
  }

  .autosave-status.pending,
  .autosave-status.saving {
    border-color: color-mix(in srgb, var(--agent-accent, #06b6d4) 45%, var(--surface-border, #334155));
    color: var(--agent-accent, #06b6d4);
  }

  .autosave-status.saved {
    border-color: color-mix(in srgb, var(--success-green, #22c55e) 45%, var(--surface-border, #334155));
    color: var(--success-green, #22c55e);
  }

  .autosave-status.error {
    border-color: color-mix(in srgb, var(--alert-red, #ef4444) 55%, var(--surface-border, #334155));
    color: var(--alert-red, #ef4444);
    background: color-mix(in srgb, var(--alert-red, #ef4444) 10%, transparent);
  }

  /* Config sections */
  .config-section {
    margin-bottom: 12px;
  }

  .section-header {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    border: none;
    background: transparent;
    cursor: pointer;
    padding: 4px 0;
    margin-bottom: 8px;
  }

  .section-label {
    font-size: 0.714rem;
    text-transform: uppercase;
    letter-spacing: 1px;
    color: var(--text-muted, #64748b);
    flex: 1;
    text-align: left;
  }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .chevron {
    width: 0;
    height: 0;
    border-left: 4px solid transparent;
    border-right: 4px solid transparent;
    border-top: 5px solid var(--text-muted, #64748b);
    transition: transform 0.15s ease;
    flex-shrink: 0;
  }

  .chevron.collapsed {
    transform: rotate(-90deg);
  }

  .section-body {
    padding-left: 12px;
  }

  /* Fields */
  .config-field {
    margin-bottom: 8px;
  }

  .config-field label {
    display: block;
    font-size: 0.786rem;
    color: var(--text-secondary, #94a3b8);
    margin-bottom: 3px;
  }

  .config-field input,
  .config-field textarea {
    width: 100%;
    background: var(--card-bg, #1e293b);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 6px;
    padding: 6px 8px;
    font-size: 0.857rem;
    color: var(--text-primary, #e2e8f0);
    font-family: inherit;
    outline: none;
    transition: border-color 0.15s;
  }

  .config-field input:focus,
  .config-field textarea:focus {
    border-color: var(--agent-accent, #06b6d4);
  }

  .config-field input[type="number"] {
    appearance: textfield;
    -moz-appearance: textfield;
  }

  /* System prompt textarea */
  .system-prompt {
    min-height: 120px;
    font-family: var(--mono-family, 'JetBrains Mono', monospace) !important;
    font-size: 0.786rem !important;
    resize: vertical;
    line-height: 1.5;
  }

  /* Color swatches */
  .color-swatches {
    display: flex;
    gap: 6px;
    margin-top: 2px;
  }

  .swatch {
    width: 20px;
    height: 20px;
    border-radius: 4px;
    border: 2px solid transparent;
    cursor: pointer;
    padding: 0;
    transition: border-color 0.15s;
  }

  .swatch.selected {
    border-color: var(--text-primary, #e2e8f0);
  }

  .swatch:hover:not(.selected) {
    border-color: var(--text-muted, #64748b);
  }

  /* Connection type buttons */
  .type-buttons {
    display: flex;
    gap: 6px;
    margin-top: 2px;
  }

  .type-btn {
    background: transparent;
    border: 1px solid var(--surface-border, #334155);
    color: var(--text-muted, #64748b);
    padding: 4px 12px;
    border-radius: 6px;
    font-size: 0.786rem;
    cursor: pointer;
    transition: all 0.15s;
  }

  .type-btn.active {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 12%, transparent);
    border-color: var(--agent-accent, #06b6d4);
    color: var(--agent-accent, #06b6d4);
  }

  .type-btn:hover:not(.active) {
    border-color: var(--text-muted, #64748b);
    color: var(--text-secondary, #94a3b8);
  }

  /* Test connection */
  .test-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-top: 4px;
  }

  .btn-test {
    background: transparent;
    border: 1px solid var(--agent-accent, #06b6d4);
    color: var(--agent-accent, #06b6d4);
    padding: 4px 12px;
    border-radius: 6px;
    font-size: 0.786rem;
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-test:hover {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 12%, transparent);
  }

  .btn-test:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .btn-folder {
    padding: 4px 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .test-status {
    font-size: 0.714rem;
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .test-status.pass {
    color: var(--success-green, #22c55e);
  }

  .test-status.pass::before {
    content: '';
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--success-green, #22c55e);
  }

  .test-status.fail {
    color: var(--alert-red, #ef4444);
  }

  .test-status.fail::before {
    content: '';
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--alert-red, #ef4444);
  }

  /* Field row (side by side) */
  .field-row {
    display: flex;
    gap: 6px;
  }

  /* Toggle rows */
  .toggle-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 6px 0;
    font-size: 0.857rem;
    color: var(--text-primary, #e2e8f0);
    border-bottom: 1px solid var(--card-bg, #1e293b);
  }

  .toggle {
    width: 32px;
    height: 18px;
    background: var(--surface-border, #334155);
    border-radius: 9px;
    position: relative;
    cursor: pointer;
    border: none;
    padding: 0;
    transition: background 0.15s;
    flex-shrink: 0;
  }

  .toggle.on {
    background: var(--agent-accent, #06b6d4);
  }

  .toggle::after {
    content: '';
    position: absolute;
    width: 14px;
    height: 14px;
    background: var(--text-primary, #e2e8f0);
    border-radius: 50%;
    top: 2px;
    left: 2px;
    transition: transform 0.15s;
  }

  .toggle.on::after {
    transform: translateX(14px);
  }

  /* Limits grid */
  .limits-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
  }

  /* Approval note */
  .approval-note {
    font-size: 0.786rem;
    color: var(--text-muted, #64748b);
    line-height: 1.5;
  }

  /* Personality section */
  .provenance {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 0.786rem;
    margin-bottom: 12px;
    flex-wrap: wrap;
  }

  .provenance-label {
    color: var(--text-muted, #64748b);
  }

  .provenance-value {
    color: var(--text-primary, #e2e8f0);
    text-transform: capitalize;
  }

  .provenance-blend {
    color: var(--text-secondary, #94a3b8);
    text-transform: capitalize;
  }

  .provenance-spec {
    color: var(--text-secondary, #94a3b8);
    text-transform: capitalize;
  }

  .radar-container {
    display: flex;
    justify-content: center;
    padding: 8px 0;
  }

  .ocean-readout {
    display: flex;
    justify-content: center;
    gap: 12px;
    margin-bottom: 12px;
  }

  .ocean-value {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
  }

  .ocean-label {
    font-size: 0.643rem;
    font-weight: 600;
    color: var(--text-muted, #64748b);
    text-transform: uppercase;
  }

  .ocean-num {
    font-size: 0.857rem;
    color: var(--text-primary, #e2e8f0);
    font-variant-numeric: tabular-nums;
  }

  .comm-controls {
    display: flex;
    gap: 8px;
    margin-bottom: 12px;
  }

  .comm-item {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 0.714rem;
    color: var(--text-secondary, #94a3b8);
  }

  .comm-item select {
    background: var(--card-bg, #1e293b);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 6px;
    padding: 4px 6px;
    font-size: 0.786rem;
    color: var(--text-primary, #e2e8f0);
    font-family: inherit;
    outline: none;
    cursor: pointer;
  }

  .comm-item select:focus {
    border-color: var(--agent-accent, #06b6d4);
  }

  .magnitude-warning {
    font-size: 0.714rem;
    color: var(--alert-yellow, #f59e0b);
    background: color-mix(in srgb, var(--alert-yellow, #f59e0b) 8%, transparent);
    border: 1px solid color-mix(in srgb, var(--alert-yellow, #f59e0b) 25%, transparent);
    border-radius: 6px;
    padding: 8px 10px;
    margin-bottom: 8px;
    line-height: 1.4;
  }

  .traits-actions {
    display: flex;
    gap: 8px;
    margin-bottom: 12px;
  }

  .btn-save {
    flex: 1;
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 15%, transparent);
    border: 1px solid var(--agent-accent, #06b6d4);
    color: var(--agent-accent, #06b6d4);
    padding: 6px 12px;
    border-radius: 6px;
    font-size: 0.786rem;
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-save:hover:not(:disabled) {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 25%, transparent);
  }

  .btn-save:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .btn-save.confirming {
    background: color-mix(in srgb, var(--alert-yellow, #f59e0b) 15%, transparent);
    border-color: var(--alert-yellow, #f59e0b);
    color: var(--alert-yellow, #f59e0b);
  }

  .btn-save.confirming:hover {
    background: color-mix(in srgb, var(--alert-yellow, #f59e0b) 25%, transparent);
  }

  .btn-reset {
    background: transparent;
    border: 1px solid var(--surface-border, #334155);
    color: var(--text-muted, #64748b);
    padding: 6px 12px;
    border-radius: 6px;
    font-size: 0.786rem;
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-reset:hover {
    border-color: var(--text-muted, #64748b);
    color: var(--text-secondary, #94a3b8);
  }

  .changelog-section {
    margin-top: 12px;
  }

  .changelog-header {
    display: block;
    font-size: 0.714rem;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted, #64748b);
    margin-bottom: 6px;
  }

  .changelog-viewer {
    max-height: 200px;
    overflow-y: auto;
    background: var(--card-bg, #1e293b);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 6px;
    padding: 8px 10px;
  }

  .changelog-viewer pre {
    margin: 0;
    font-size: 0.714rem;
    line-height: 1.5;
    color: var(--text-secondary, #94a3b8);
    white-space: pre-wrap;
    word-break: break-word;
    font-family: var(--mono-family, 'JetBrains Mono', monospace);
  }
</style>
