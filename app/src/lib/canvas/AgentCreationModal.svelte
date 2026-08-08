<script lang="ts">
  import { onMount } from 'svelte';
  import ModalShell from '$lib/modals/ModalShell.svelte';
  import Input from '$lib/ui/Input.svelte';
  import Select from '$lib/ui/Select.svelte';
  import Toggle from '$lib/ui/Toggle.svelte';
  import Button from '$lib/ui/Button.svelte';
  import Textarea from '$lib/ui/Textarea.svelte';
  import OceanRadar from '$lib/ui/OceanRadar.svelte';
  import {
    createAgent,
    storeApiKey,
    listConnections,
    scanLocalModels,
    getWorkspaceRoot,
    pickFolder,
    isClaudeCodeAvailable,
    listArchetypes,
    type ConnectionInfo,
    type DetectedModel,
    type AgentColor,
    type ArchetypeBaseSummary,
    type ArchetypeSpecSummary,
    type OceanScores,
  } from '$lib/tauri/commands';

  import { AGENT_COLOR_PRESETS, DEFAULT_SYSTEM_PROMPT } from '$lib/constants';

  interface Props {
    x: number;
    y: number;
    onclose: () => void;
    oncreated: (agentId: string, x: number, y: number) => void;
  }

  let { x, y, onclose, oncreated }: Props = $props();

  // --- Preset colors ---
  const presetColors: AgentColor[] = AGENT_COLOR_PRESETS;

  // --- Form state ---
  let name = $state('');
  let connectionType = $state<'api' | 'local' | 'mcp_stdio' | 'codex' | 'claude_code'>('api');
  let model = $state('');
  let endpoint = $state('');
  let host = $state('localhost');
  let port = $state('11434');
  let mcpCommand = $state('');
  let workingDirectory = $state('');
  let selectedColor = $state<AgentColor>(presetColors[0]);
  let apiKey = $state('');
  let systemPrompt = $state(DEFAULT_SYSTEM_PROMPT);
  let codexSessionMode = $state<'fresh' | 'resume'>('fresh');
  let codexSessionId = $state('');

  // Archetype state
  let archetypeBases = $state<ArchetypeBaseSummary[]>([]);
  let archetypeSpecs = $state<ArchetypeSpecSummary[]>([]);
  let selectedBase = $state('');
  let selectedSpec = $state('');

  // Tool toggles
  let toolsFilesystem = $state(true);
  let toolsCodeExecution = $state(true);
  let toolsWebAccess = $state(false);
  let toolsVerification = $state(true);
  let toolsSubagent = $state(true);
  let toolsA2a = $state(false);

  // UI state
  let submitting = $state(false);
  let errorMessage = $state('');
  let toolsOpen = $state(false);

  // Loaded data
  let savedConnections = $state<ConnectionInfo[]>([]);
  let localModels = $state<DetectedModel[]>([]);
  let selectedConnectionId = $state('__manual__');

  // Claude Code SDK availability
  let claudeCodeAvailable = $state<boolean | null>(null);
  let claudeCodeError = $state('');

  // --- Derived state ---
  let cloudConnections = $derived(savedConnections.filter((c) => !c.is_local));

  let selectedConnection = $derived(
    selectedConnectionId !== '__manual__'
      ? savedConnections.find((c) => c.id === selectedConnectionId) ?? null
      : null,
  );

  let showApiKeyInput = $derived(
    connectionType === 'api' &&
      (selectedConnectionId === '__manual__' ||
        (selectedConnection !== null && !selectedConnection.key_ref)),
  );

  let isCustomArchetype = $derived(!selectedBase);
  let isSdkAgent = $derived(connectionType === 'claude_code');

  // Preview panel data
  let selectedBaseData = $derived(
    archetypeBases.find((b) => b.id === selectedBase) ?? null,
  );

  let selectedSpecData = $derived(
    archetypeSpecs.find((s) => s.id === selectedSpec) ?? null,
  );

  // (resolvedOcean and showPreview are now defined below with blending support)

  /** Human-readable OCEAN trait descriptions */
  function traitLabel(key: string, value: number): string {
    if (key === 'openness') {
      return value > 0.6 ? 'Creative, seeks novelty' : value > 0.4 ? 'Balanced approach' : 'Prefers proven methods';
    }
    if (key === 'conscientiousness') {
      return value > 0.6 ? 'Methodical, detail-oriented' : value > 0.4 ? 'Pragmatic balance' : 'Fast and flexible';
    }
    if (key === 'extraversion') {
      return value > 0.6 ? 'Proactive communicator' : value > 0.4 ? 'Engages when relevant' : 'Focused, speaks when needed';
    }
    if (key === 'agreeableness') {
      return value > 0.6 ? 'Consensus-seeking, diplomatic' : value > 0.4 ? 'Collaborative with pushback' : 'Direct, challenges openly';
    }
    if (key === 'neuroticism') {
      return value > 0.6 ? 'Risk-aware, thorough checker' : value > 0.4 ? 'Appropriately cautious' : 'Steady under pressure';
    }
    return '';
  }

  // ── Secondary base blending ──────────────────────────────────────
  let secondaryBase = $state('');
  let blendWeight = $state(70); // 50-100% primary

  let secondaryBaseData = $derived(
    archetypeBases.find((b) => b.id === secondaryBase) ?? null,
  );

  // ── Communication style controls ──────────────────────────────────
  let verbosity = $state('balanced');
  let initiative = $state('measured');
  let tone = $state('balanced');

  // Populate communication defaults from primary base
  $effect(() => {
    if (selectedBaseData) {
      // Only set defaults, don't override if user has changed them
      verbosity = 'balanced';
      initiative = 'measured';
      tone = 'balanced';
    }
  });

  // ── Manual OCEAN overrides (from interactive radar drag) ──────────
  let manualOverrides = $state<Record<string, number>>({});

  // Recompute resolved OCEAN with blending + overrides
  let resolvedOcean = $derived.by<OceanScores | null>(() => {
    if (!selectedBaseData && !selectedBase) {
      // Custom: start at 0.5 across the board, apply overrides
      return {
        openness: manualOverrides.openness ?? 0.5,
        conscientiousness: manualOverrides.conscientiousness ?? 0.5,
        extraversion: manualOverrides.extraversion ?? 0.5,
        agreeableness: manualOverrides.agreeableness ?? 0.5,
        neuroticism: manualOverrides.neuroticism ?? 0.5,
      };
    }
    if (!selectedBaseData) return null;

    let base = { ...selectedBaseData.ocean };

    // Blend with secondary if present
    if (secondaryBaseData) {
      const w = blendWeight / 100;
      const sw = 1 - w;
      base = {
        openness: base.openness * w + secondaryBaseData.ocean.openness * sw,
        conscientiousness: base.conscientiousness * w + secondaryBaseData.ocean.conscientiousness * sw,
        extraversion: base.extraversion * w + secondaryBaseData.ocean.extraversion * sw,
        agreeableness: base.agreeableness * w + secondaryBaseData.ocean.agreeableness * sw,
        neuroticism: base.neuroticism * w + secondaryBaseData.ocean.neuroticism * sw,
      };
    }

    // Apply spec modifiers
    if (selectedSpecData) {
      const mod = selectedSpecData.ocean_modifiers;
      base.openness = Math.min(1, Math.max(0, base.openness + mod.openness));
      base.conscientiousness = Math.min(1, Math.max(0, base.conscientiousness + mod.conscientiousness));
      base.extraversion = Math.min(1, Math.max(0, base.extraversion + mod.extraversion));
      base.agreeableness = Math.min(1, Math.max(0, base.agreeableness + mod.agreeableness));
      base.neuroticism = Math.min(1, Math.max(0, base.neuroticism + mod.neuroticism));
    }

    // Apply manual overrides
    return {
      openness: manualOverrides.openness ?? base.openness,
      conscientiousness: manualOverrides.conscientiousness ?? base.conscientiousness,
      extraversion: manualOverrides.extraversion ?? base.extraversion,
      agreeableness: manualOverrides.agreeableness ?? base.agreeableness,
      neuroticism: manualOverrides.neuroticism ?? base.neuroticism,
    };
  });

  function handleRadarChange(trait: keyof OceanScores, value: number) {
    manualOverrides = { ...manualOverrides, [trait]: value };
  }

  // Show preview for any non-SDK agent (including Custom)
  let showPreview = $derived(!isSdkAgent && (selectedBaseData !== null || selectedBase === 'custom' || selectedBase === ''));

  let baseOptions = $derived([
    { value: '', label: 'Custom' },
    ...archetypeBases.map((b) => ({ value: b.id, label: b.name })),
  ]);

  let secondaryBaseOptions = $derived([
    { value: '', label: 'None' },
    ...archetypeBases
      .filter((b) => b.id !== selectedBase)
      .map((b) => ({ value: b.id, label: b.name })),
  ]);

  let specOptions = $derived(() => {
    if (!selectedBase) return [];
    // Group by department with separator-style labels
    const grouped = new Map<string, ArchetypeSpecSummary[]>();
    for (const spec of archetypeSpecs) {
      const dept = spec.department;
      if (!grouped.has(dept)) grouped.set(dept, []);
      grouped.get(dept)!.push(spec);
    }
    const options: Array<{ value: string; label: string }> = [];
    for (const [dept, specs] of [...grouped.entries()].sort((a, b) => a[0].localeCompare(b[0]))) {
      const deptLabel = dept.charAt(0).toUpperCase() + dept.slice(1);
      for (const spec of specs.sort((a, b) => a.name.localeCompare(b.name))) {
        options.push({ value: spec.id, label: `${deptLabel} — ${spec.name}` });
      }
    }
    return options;
  });

  let taglinePreview = $derived(() => {
    if (selectedSpec) {
      const spec = archetypeSpecs.find((s) => s.id === selectedSpec);
      if (spec?.tagline) return spec.tagline;
    }
    if (selectedBase) {
      const base = archetypeBases.find((b) => b.id === selectedBase);
      if (base?.tagline) return base.tagline;
    }
    return '';
  });

  // Connection type options for segmented bar
  const connectionTypes = [
    { value: 'api' as const, label: 'Cloud API' },
    { value: 'local' as const, label: 'Local' },
    { value: 'mcp_stdio' as const, label: 'MCP' },
    { value: 'codex' as const, label: 'Codex' },
    { value: 'claude_code' as const, label: 'Claude Code' },
  ];

  // --- Effects ---

  // Reset specialization when base changes
  $effect(() => {
    // Access selectedBase to subscribe
    const _ = selectedBase;
    selectedSpec = '';
  });

  // Auto-fill endpoint from selected connection
  $effect(() => {
    if (selectedConnection) {
      endpoint = selectedConnection.endpoint;
    }
  });

  // --- Lifecycle ---
  onMount(async () => {
    const results = await Promise.allSettled([
      listConnections(),
      scanLocalModels(),
      getWorkspaceRoot(),
      listArchetypes(),
      isClaudeCodeAvailable(),
    ]);

    if (results[0].status === 'fulfilled') savedConnections = results[0].value;
    if (results[1].status === 'fulfilled') localModels = results[1].value;
    if (results[2].status === 'fulfilled') workingDirectory = results[2].value;
    if (results[3].status === 'fulfilled') {
      archetypeBases = results[3].value.bases;
      archetypeSpecs = results[3].value.specializations;
    }
    if (results[4].status === 'fulfilled') {
      claudeCodeAvailable = results[4].value;
    } else {
      claudeCodeAvailable = false;
      claudeCodeError = String((results[4] as PromiseRejectedResult).reason);
    }
  });

  // --- Handlers ---
  async function handlePickFolder() {
    try {
      const picked = await pickFolder(workingDirectory || undefined);
      if (picked) workingDirectory = picked;
    } catch {
      // User cancelled dialog
    }
  }

  async function handleSubmit() {
    if (!name.trim()) {
      errorMessage = 'Agent name is required.';
      return;
    }
    if (connectionType === 'codex' && codexSessionMode === 'resume' && !codexSessionId.trim()) {
      errorMessage = 'Session ID is required to resume a Codex session.';
      return;
    }

    submitting = true;
    errorMessage = '';

    try {
      const baseParams = {
        name: name.trim(),
        workingDirectory: workingDirectory.trim() || '~',
        colorR: selectedColor.r,
        colorG: selectedColor.g,
        colorB: selectedColor.b,
        archetypeBase: selectedBase || undefined,
        archetypeSecondaryBase: secondaryBase || undefined,
        archetypeBlendWeight: secondaryBase ? blendWeight / 100 : undefined,
        archetypeSpecialization: selectedSpec || undefined,
        commVerbosity: verbosity !== 'balanced' ? verbosity : undefined,
        commInitiative: initiative !== 'measured' ? initiative : undefined,
        commTone: tone !== 'balanced' ? tone : undefined,
        oceanOverrides: Object.keys(manualOverrides).length > 0 ? manualOverrides : undefined,
        toolsFilesystem,
        toolsCodeExecution,
        toolsWebAccess,
        toolsVerification,
        toolsSubagent,
        toolsA2aMessaging: toolsA2a,
      };

      let params;
      if (connectionType === 'api') {
        const ep = selectedConnection?.endpoint || endpoint.trim();
        const keyRef = selectedConnection?.key_ref || undefined;
        params = { ...baseParams, connectionType: 'api', endpoint: ep || undefined, model: model.trim() || undefined, keyRef };
      } else if (connectionType === 'local') {
        params = { ...baseParams, connectionType: 'local', host: host.trim() || undefined, port: parseInt(port) || undefined, model: model.trim() || undefined };
      } else if (connectionType === 'mcp_stdio') {
        params = { ...baseParams, connectionType: 'mcp_stdio', endpoint: mcpCommand.trim() || undefined };
      } else if (connectionType === 'codex') {
        params = {
          ...baseParams,
          connectionType: 'codex',
          model: model.trim() || undefined,
          agentSessionId: codexSessionMode === 'resume' ? (codexSessionId.trim() || undefined) : undefined,
        };
      } else if (connectionType === 'claude_code') {
        params = { ...baseParams, connectionType: 'claude_code' };
      } else {
        throw new Error(`Unsupported connection type: ${connectionType satisfies never}`);
      }

      const newAgentId = await createAgent(params);

      // Store API key if user entered one
      if (connectionType === 'api' && apiKey.trim()) {
        try {
          await storeApiKey(newAgentId, apiKey.trim());
        } catch (keyErr) {
          console.warn('Failed to store API key:', keyErr);
        }
      }

      oncreated(newAgentId, x, y);
    } catch (err) {
      errorMessage = err instanceof Error ? err.message : String(err);
    } finally {
      submitting = false;
    }
  }
</script>

<ModalShell title="New Agent" width={showPreview ? '780px' : '540px'} onclose={onclose}>
  <div class="modal-layout" class:has-preview={showPreview}>
    <!-- Left: Form -->
    <div class="form-content">
    <!-- Section: Identity -->
    <section class="section">
      <Input label="Name" bind:value={name} placeholder="My Agent" />

      {#if !isSdkAgent}
        <div class="row">
          <div class="row-item">
            <Select label="Cognitive Base" bind:value={selectedBase} options={baseOptions} />
          </div>
          <div class="row-item">
            <Select
              label="Specialization"
              bind:value={selectedSpec}
              options={specOptions()}
            />
          </div>
        </div>

        {#if selectedBase}
          <!-- Secondary base blending -->
          <div class="blend-row">
            <div class="row-item">
              <Select label="Blend with" bind:value={secondaryBase} options={secondaryBaseOptions} />
            </div>
            {#if secondaryBase}
              <div class="row-item blend-slider">
                <!-- svelte-ignore a11y_label_has_associated_control -->
                <label class="slider-label">
                  Weight: {blendWeight}% {selectedBaseData?.name ?? ''} / {100 - blendWeight}% {secondaryBaseData?.name ?? ''}
                </label>
                <input type="range" min="50" max="100" bind:value={blendWeight} class="weight-range" />
              </div>
            {/if}
          </div>
        {/if}

        <!-- Communication style controls -->
        <div class="comm-controls">
          <div class="comm-item">
            <Select label="Verbosity" bind:value={verbosity} options={[
              { value: 'terse', label: 'Terse' },
              { value: 'concise', label: 'Concise' },
              { value: 'balanced', label: 'Balanced' },
              { value: 'thorough', label: 'Thorough' },
              { value: 'verbose', label: 'Verbose' },
            ]} />
          </div>
          <div class="comm-item">
            <Select label="Initiative" bind:value={initiative} options={[
              { value: 'reactive', label: 'Reactive' },
              { value: 'measured', label: 'Measured' },
              { value: 'proactive', label: 'Proactive' },
            ]} />
          </div>
          <div class="comm-item">
            <Select label="Tone" bind:value={tone} options={[
              { value: 'direct', label: 'Direct' },
              { value: 'balanced', label: 'Balanced' },
              { value: 'warm', label: 'Warm' },
            ]} />
          </div>
        </div>

        {#if taglinePreview()}
          <p class="tagline">{taglinePreview()}</p>
        {/if}
      {/if}
    </section>

    <!-- Custom archetype note -->
    {#if isCustomArchetype && !isSdkAgent}
      <div class="info-note">
        Custom agents start with neutral traits. On first interaction, a guided onboarding
        conversation establishes core directives.
      </div>
    {/if}

    <!-- Section: Connection -->
    <section class="section">
      <span class="section-label">CONNECTION</span>
      <div class="segmented-bar">
        {#each connectionTypes as ct}
          <button
            type="button"
            class="seg-btn"
            class:active={connectionType === ct.value}
            disabled={ct.value === 'claude_code' && claudeCodeAvailable === false}
            title={ct.value === 'claude_code' && claudeCodeAvailable === false ? (claudeCodeError || 'Claude CLI not found') : ''}
            onclick={() => { connectionType = ct.value; }}
          >
            {ct.label}
          </button>
        {/each}
      </div>

      <!-- Claude Code info -->
      {#if connectionType === 'claude_code'}
        <div class="info-note">
          Uses your Claude Pro/Max subscription via Claude CLI. No API key needed.
        </div>
      {/if}

      <!-- Codex fields -->
      {#if connectionType === 'codex'}
        <div class="info-note">
          Uses your local Codex OAuth credentials from <span class="mono">~/.codex/auth.json</span>.
        </div>
        <Input label="Model" bind:value={model} placeholder="gpt-5.5" />
        <div class="sub-section">
          <span class="section-label">SESSION</span>
          <div class="segmented-bar small">
            <button
              type="button"
              class="seg-btn"
              class:active={codexSessionMode === 'fresh'}
              onclick={() => { codexSessionMode = 'fresh'; }}
            >Fresh</button>
            <button
              type="button"
              class="seg-btn"
              class:active={codexSessionMode === 'resume'}
              onclick={() => { codexSessionMode = 'resume'; }}
            >Resume</button>
          </div>
          {#if codexSessionMode === 'resume'}
            <Input label="Session ID" bind:value={codexSessionId} placeholder="019db81a-32f7-7440-baee-b321402ccf9e" />
          {/if}
        </div>
      {/if}

      <!-- Cloud API fields -->
      {#if connectionType === 'api'}
        <Select
          label="Connection"
          bind:value={selectedConnectionId}
          options={[
            { value: '__manual__', label: 'Enter manually' },
            ...cloudConnections.map((c) => ({
              value: c.id,
              label: `${c.provider} (${c.endpoint.replace('https://', '').split('/')[0]})`,
            })),
          ]}
        />

        {#if selectedConnectionId === '__manual__'}
          <Input label="Endpoint URL" bind:value={endpoint} placeholder="https://api.openai.com/v1" />
        {:else if selectedConnection}
          <Input label="Endpoint" value={selectedConnection.endpoint} disabled />
        {/if}

        <Input label="Model" bind:value={model} placeholder="gpt-4o" />

        {#if showApiKeyInput}
          <Input label="API Key" bind:value={apiKey} type="password" placeholder="sk-..." />
          <p class="hint">Stored securely in OS keychain</p>
        {/if}
      {/if}

      <!-- Local fields -->
      {#if connectionType === 'local'}
        <div class="row">
          <div class="row-item">
            <Input label="Host" bind:value={host} placeholder="localhost" />
          </div>
          <div class="row-port">
            <Input label="Port" bind:value={port} type="number" placeholder="11434" />
          </div>
        </div>

        {#if localModels.length > 0}
          <Select
            label="Model"
            bind:value={model}
            options={[
              { value: '', label: 'Select a model...' },
              ...localModels.flatMap((service) =>
                service.models.map((m) => ({
                  value: m,
                  label: `${m} (${service.service_name}:${service.port})`,
                })),
              ),
            ]}
          />
        {:else}
          <Input label="Model" bind:value={model} placeholder="llama3 (no local models detected)" />
        {/if}
      {/if}

      <!-- MCP fields -->
      {#if connectionType === 'mcp_stdio'}
        <Input label="Command" bind:value={mcpCommand} placeholder="npx @modelcontextprotocol/server" />
      {/if}
    </section>

    <!-- Section: Working Directory -->
    <section class="section">
      <span class="section-label">WORKING DIRECTORY</span>
      <div class="dir-row">
        <div class="dir-input">
          <Input bind:value={workingDirectory} placeholder="~" />
        </div>
        <Button variant="icon" onclick={handlePickFolder}>
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
            <path
              d="M2 3.5A1.5 1.5 0 013.5 2h2.879a1.5 1.5 0 011.06.44l.622.62a.5.5 0 00.354.147H12.5A1.5 1.5 0 0114 4.707V12.5a1.5 1.5 0 01-1.5 1.5h-9A1.5 1.5 0 012 12.5v-9z"
              stroke="currentColor"
              stroke-width="1.2"
            />
          </svg>
        </Button>
      </div>
    </section>

    <!-- Section: Color -->
    <section class="section">
      <span class="section-label">COLOR</span>
      <div class="color-palette">
        {#each presetColors as c}
          <button
            type="button"
            class="color-swatch"
            class:selected={selectedColor.r === c.r && selectedColor.g === c.g && selectedColor.b === c.b}
            style="background: rgb({c.r}, {c.g}, {c.b})"
            onclick={() => (selectedColor = c)}
            aria-label="Color {c.r},{c.g},{c.b}"
          ></button>
        {/each}
      </div>
    </section>

    <!-- Section: Tools (collapsible) -->
    <section class="section">
      <button type="button" class="collapsible-header" onclick={() => (toolsOpen = !toolsOpen)}>
        <span class="chevron" class:open={toolsOpen}>&#9656;</span>
        <span class="section-label">TOOLS</span>
      </button>
      {#if toolsOpen}
        <div class="tools-grid">
          <Toggle label="Filesystem" bind:checked={toolsFilesystem} />
          <Toggle label="Code Execution" bind:checked={toolsCodeExecution} />
          <Toggle label="Web Access" bind:checked={toolsWebAccess} />
          <Toggle label="Verification" bind:checked={toolsVerification} />
          <Toggle label="Subagent" bind:checked={toolsSubagent} />
          <Toggle label="A2A Messaging" bind:checked={toolsA2a} />
        </div>
      {/if}
    </section>

    <!-- Error -->
    {#if errorMessage}
      <div class="error-message">{errorMessage}</div>
    {/if}

    <!-- Actions -->
    <div class="actions">
      <Button variant="secondary" onclick={onclose} disabled={submitting}>Cancel</Button>
      <Button variant="primary" onclick={handleSubmit} disabled={submitting || !name.trim()}>
        {submitting ? 'Creating...' : 'Create Agent'}
      </Button>
    </div>
    </div>

    <!-- Right: Preview Panel -->
    {#if showPreview && resolvedOcean}
      <aside class="preview-panel">
        <div class="preview-header">
          {#if selectedBaseData}
            <span class="preview-name">{selectedBaseData.name}</span>
            {#if secondaryBaseData}
              <span class="preview-blend">+ {secondaryBaseData.name}</span>
            {/if}
            {#if selectedSpecData}
              <span class="preview-spec">/ {selectedSpecData.name}</span>
            {/if}
          {:else}
            <span class="preview-name">Custom</span>
          {/if}
        </div>

        <p class="preview-description">
          {selectedSpecData?.description ?? selectedBaseData?.description ?? 'Neutral personality — drag the radar points to sculpt traits.'}
        </p>

        <div class="preview-radar">
          <OceanRadar
            scores={resolvedOcean}
            overlay={selectedBaseData && selectedSpecData ? selectedBaseData.ocean : undefined}
            size={160}
            interactive
            onchange={handleRadarChange}
          />
        </div>

        <div class="preview-traits">
          {#each Object.entries(resolvedOcean) as [key, value]}
            {@const isOverridden = key in manualOverrides}
            <div class="trait-row" class:overridden={isOverridden}>
              <span class="trait-key">{key.charAt(0).toUpperCase()}</span>
              <span class="trait-bar-wrap">
                <span class="trait-bar" style="width: {value * 100}%"></span>
              </span>
              <span class="trait-label">{traitLabel(key, value)}</span>
            </div>
          {/each}
        </div>

        {#if Object.keys(manualOverrides).length > 0}
          <button
            type="button"
            class="reset-overrides"
            onclick={() => { manualOverrides = {}; }}
          >
            Reset manual adjustments
          </button>
        {/if}
      </aside>
    {/if}
  </div>
</ModalShell>

<style>
  .modal-layout {
    display: flex;
  }

  .modal-layout.has-preview {
    gap: 0;
  }

  .form-content {
    padding: 20px 24px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
    min-width: 0;
  }

  /* --- Preview Panel --- */
  .preview-panel {
    width: 240px;
    flex-shrink: 0;
    padding: 20px 16px;
    border-left: var(--border-width, 1px) solid var(--surface-border);
    background: color-mix(in srgb, var(--surface-border) 8%, var(--card-bg));
    display: flex;
    flex-direction: column;
    gap: 14px;
    overflow-y: auto;
  }

  .preview-header {
    display: flex;
    align-items: baseline;
    gap: 4px;
    flex-wrap: wrap;
  }

  .preview-name {
    font-size: 1rem;
    font-weight: 600;
    color: var(--text-primary);
  }

  .preview-spec {
    font-size: 0.929rem;
    color: var(--text-muted);
  }

  .preview-description {
    font-size: 0.857rem;
    color: var(--text-secondary);
    line-height: 1.5;
    margin: 0;
  }

  .preview-radar {
    display: flex;
    justify-content: center;
  }

  .preview-traits {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .trait-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .trait-key {
    font-size: 0.786rem;
    font-weight: 700;
    color: var(--text-secondary);
    width: 12px;
    text-align: center;
    flex-shrink: 0;
  }

  .trait-bar-wrap {
    width: 36px;
    height: 4px;
    background: var(--surface-border);
    border-radius: 2px;
    flex-shrink: 0;
    overflow: hidden;
  }

  .trait-bar {
    display: block;
    height: 100%;
    background: var(--infra-accent);
    border-radius: 2px;
    transition: width var(--transition-speed, 200ms) ease;
  }

  .trait-label {
    font-size: 0.786rem;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-bottom: 16px;
  }

  .section-label {
    font-size: 0.714rem;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-weight: var(--body-weight, 400);
  }

  .sub-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 4px;
  }

  /* --- Row layouts --- */
  .row {
    display: flex;
    gap: 10px;
    align-items: flex-start;
  }

  .row-item {
    flex: 1;
    min-width: 0;
  }

  .row-port {
    width: 100px;
    flex-shrink: 0;
  }

  .dir-row {
    display: flex;
    gap: 6px;
    align-items: flex-start;
  }

  .dir-input {
    flex: 1;
    min-width: 0;
  }

  /* --- Segmented button bar --- */
  .segmented-bar {
    display: flex;
    border: var(--border-width, 1px) var(--border-style, solid) var(--surface-border);
    border-radius: var(--border-radius, 6px);
    overflow: hidden;
  }

  .seg-btn {
    flex: 1;
    background: transparent;
    border: none;
    border-right: var(--border-width, 1px) var(--border-style, solid) var(--surface-border);
    padding: 7px 8px;
    font-size: 0.929rem;
    font-family: var(--font-family);
    font-weight: var(--body-weight, 400);
    color: var(--text-secondary);
    cursor: pointer;
    transition: all var(--transition-speed, 200ms) ease;
    white-space: nowrap;
  }

  .seg-btn:last-child {
    border-right: none;
  }

  .seg-btn:hover:not(:disabled) {
    background: color-mix(in srgb, var(--surface-border) 20%, transparent);
    color: var(--text-primary);
  }

  .seg-btn.active {
    background: color-mix(in srgb, var(--infra-accent) 15%, transparent);
    color: var(--infra-accent);
    font-weight: 600;
  }

  .seg-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }

  .segmented-bar.small .seg-btn {
    padding: 5px 12px;
    font-size: 0.857rem;
  }

  /* --- Tagline preview --- */
  .tagline {
    font-size: 0.857rem;
    color: var(--text-muted);
    margin: -4px 0 0 0;
    font-style: italic;
    line-height: 1.4;
  }

  /* --- Info note --- */
  .info-note {
    padding: 10px 12px;
    background: color-mix(in srgb, var(--surface-border) 15%, transparent);
    border-radius: var(--border-radius, 6px);
    font-size: 0.857rem;
    color: var(--text-secondary);
    line-height: 1.4;
  }

  .mono {
    font-family: var(--mono-family);
    font-size: 0.786rem;
  }

  .hint {
    font-size: 0.714rem;
    color: var(--text-muted);
    margin: -4px 0 0 0;
  }

  /* --- Color palette --- */
  .color-palette {
    display: flex;
    gap: 6px;
  }

  .color-swatch {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    border: 2px solid transparent;
    cursor: pointer;
    transition: border-color var(--transition-speed, 200ms) ease, transform 0.1s ease;
    padding: 0;
  }

  .color-swatch:hover {
    transform: scale(1.15);
  }

  .color-swatch.selected {
    border-color: var(--text-primary);
    box-shadow: 0 0 0 2px var(--card-bg), 0 0 0 4px currentColor;
  }

  /* --- Collapsible tools --- */
  .collapsible-header {
    display: flex;
    align-items: center;
    gap: 6px;
    background: none;
    border: none;
    cursor: pointer;
    padding: 0;
    color: var(--text-secondary);
    transition: color var(--transition-speed, 200ms) ease;
  }

  .collapsible-header:hover {
    color: var(--text-primary);
  }

  .chevron {
    display: inline-block;
    font-size: 0.714rem;
    transition: transform var(--transition-speed, 200ms) ease;
  }

  .chevron.open {
    transform: rotate(90deg);
  }

  .tools-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px 20px;
    padding: 8px 0 0 0;
  }

  /* --- Error --- */
  .error-message {
    font-size: 0.857rem;
    color: var(--alert-accent);
    background: color-mix(in srgb, var(--alert-accent) 10%, transparent);
    border: var(--border-width, 1px) solid color-mix(in srgb, var(--alert-accent) 30%, transparent);
    border-radius: var(--border-radius, 6px);
    padding: 8px 12px;
  }

  /* --- Actions --- */
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 8px;
    padding-top: 12px;
    border-top: var(--border-width, 1px) solid var(--surface-border);
  }

  /* --- Blending controls --- */
  .blend-row {
    display: flex;
    gap: 12px;
    align-items: flex-end;
  }

  .blend-slider {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .slider-label {
    font-size: 0.75rem;
    color: var(--text-secondary);
  }

  .weight-range {
    width: 100%;
    accent-color: var(--infra-accent);
  }

  /* --- Communication controls --- */
  .comm-controls {
    display: flex;
    gap: 8px;
  }

  .comm-item {
    flex: 1;
  }

  /* --- Override indicator --- */
  .trait-row.overridden .trait-key {
    color: var(--infra-accent);
    font-weight: 700;
  }

  /* --- Preview blend label --- */
  .preview-blend {
    font-size: 0.857rem;
    color: var(--text-muted);
    font-weight: 400;
  }

  /* --- Reset overrides button --- */
  .reset-overrides {
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: 0.75rem;
    cursor: pointer;
    padding: 4px 0;
    text-decoration: underline;
  }

  .reset-overrides:hover {
    color: var(--text-secondary);
  }
</style>
