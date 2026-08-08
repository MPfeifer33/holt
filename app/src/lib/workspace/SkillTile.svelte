<script lang="ts">
  import { onMount } from 'svelte';
  import {
    listSkills, getSkill, updateSkill, deleteSkill,
    assignSkillToAgent, unassignSkillFromAgent, getActiveSkills,
    fetchSkillPreview, installImportedSkill,
  } from '$lib/tauri/commands';
  import type { SkillSummary, SkillDetail, ImportPreview, ImportFile } from '$lib/tauri/commands';
  import { getAgents } from '$lib/stores/agents.svelte';

  interface Props {
    agentId: string;
  }

  const props: Props = $props();

  // --- State ---
  let allSkills = $state<SkillSummary[]>([]);
  let agentSkillNames = $state<Set<string>>(new Set());
  let loading = $state(true);
  let error = $state<string | null>(null);
  // svelte-ignore state_referenced_locally
  let activeAgentId = $state(props.agentId);
  let agents = $derived(getAgents());

  // Detail/Editor state
  let selectedSkill = $state<SkillDetail | null>(null);
  let editing = $state(false);
  let editContent = $state('');
  let saveError = $state<string | null>(null);
  let saving = $state(false);

  // Import state
  let importMode = $state(false);
  let importUrl = $state('');
  let importLoading = $state(false);
  let importPreview = $state<ImportPreview | null>(null);
  let importError = $state<string | null>(null);

  // View state
  type View = 'list' | 'detail' | 'import';
  let view = $state<View>('list');

  // --- Derived ---
  let agentSkills = $derived(allSkills.filter(s => agentSkillNames.has(s.file_name)));
  let librarySkills = $derived(allSkills.filter(s => !agentSkillNames.has(s.file_name)));

  // --- Data loading ---
  async function refresh() {
    try {
      const [skills, active] = await Promise.all([
        listSkills(),
        getActiveSkills(activeAgentId),
      ]);
      allSkills = skills;
      agentSkillNames = new Set(active.map(a => a.name));
      error = null;
    } catch (err) {
      error = String(err);
    } finally {
      loading = false;
    }
  }

  function switchAgent(newId: string) {
    activeAgentId = newId;
    refresh();
  }

  onMount(() => { refresh(); });

  // --- Actions ---
  async function handleAssign(skillName: string) {
    try {
      await assignSkillToAgent(activeAgentId, skillName);
      await refresh();
    } catch (err) { error = String(err); }
  }

  async function handleUnassign(skillName: string) {
    try {
      await unassignSkillFromAgent(activeAgentId, skillName);
      await refresh();
    } catch (err) { error = String(err); }
  }

  async function openDetail(fileName: string) {
    try {
      selectedSkill = await getSkill(fileName);
      editing = false;
      editContent = '';
      saveError = null;
      view = 'detail';
    } catch (err) { error = String(err); }
  }

  function startEdit() {
    if (!selectedSkill) return;
    editContent = selectedSkill.raw_content;
    editing = true;
  }

  async function handleSave() {
    if (!selectedSkill) return;
    saving = true;
    saveError = null;
    try {
      await updateSkill(selectedSkill.file_name, editContent);
      await refresh();
      // Reload detail
      selectedSkill = await getSkill(selectedSkill.file_name);
      editing = false;
    } catch (err) {
      saveError = String(err);
    } finally {
      saving = false;
    }
  }

  async function handleDelete() {
    if (!selectedSkill) return;
    try {
      await deleteSkill(selectedSkill.file_name);
      selectedSkill = null;
      view = 'list';
      await refresh();
    } catch (err) { error = String(err); }
  }

  function goBack() {
    view = 'list';
    selectedSkill = null;
    editing = false;
    saveError = null;
    importMode = false;
    importPreview = null;
    importError = null;
    importUrl = '';
  }

  // --- Import ---
  function openImport() {
    view = 'import';
    importUrl = '';
    importPreview = null;
    importError = null;
  }

  async function handleFetchPreview() {
    if (!importUrl.trim()) return;
    importLoading = true;
    importError = null;
    importPreview = null;
    try {
      importPreview = await fetchSkillPreview(importUrl.trim());
    } catch (err) {
      importError = String(err);
    } finally {
      importLoading = false;
    }
  }

  async function handleInstall(overwrite = false) {
    if (!importPreview) return;
    importLoading = true;
    importError = null;
    try {
      await installImportedSkill(
        importPreview.skill_name,
        importPreview.files,
        importPreview.source_url,
        importPreview.commit_sha,
        overwrite,
      );
      await refresh();
      goBack();
    } catch (err) {
      importError = String(err);
    } finally {
      importLoading = false;
    }
  }

  function modeBadgeClass(mode: string): string {
    if (mode === 'inject') return 'badge-inject';
    if (mode === 'reference') return 'badge-reference';
    return 'badge-auto';
  }
</script>

<div class="skill-tile">
  {#if view === 'detail' && selectedSkill}
    <!-- Detail/Editor View -->
    <div class="view-header">
      <button class="back-btn" onclick={goBack}>&larr; Back</button>
      <div class="header-actions">
        {#if editing}
          <button class="btn btn-save" onclick={handleSave} disabled={saving}>
            {saving ? 'Saving...' : 'Save'}
          </button>
          <button class="btn btn-cancel" onclick={() => { editing = false; saveError = null; }}>Cancel</button>
        {:else}
          <button class="btn btn-edit" onclick={startEdit}>Edit</button>
          <button class="btn btn-delete" onclick={handleDelete}>Delete</button>
        {/if}
      </div>
    </div>

    <div class="detail-meta">
      <div class="meta-row">
        <span class="meta-label">Name</span>
        <span class="meta-value">{selectedSkill.name}</span>
      </div>
      <div class="meta-row">
        <span class="meta-label">Mode</span>
        <span class="mode-badge {modeBadgeClass(selectedSkill.effective_mode)}">{selectedSkill.effective_mode}</span>
        {#if selectedSkill.mode === 'auto'}
          <span class="meta-hint">(auto)</span>
        {/if}
      </div>
      <div class="meta-row">
        <span class="meta-label">Priority</span>
        <span class="meta-value">{selectedSkill.priority}</span>
      </div>
      <div class="meta-row">
        <span class="meta-label">Tokens</span>
        <span class="meta-value">{selectedSkill.max_tokens}</span>
      </div>
      {#if selectedSkill.is_directory}
        <div class="meta-row">
          <span class="meta-label">Files</span>
          <span class="meta-value">{selectedSkill.files.length + 1}</span>
        </div>
      {/if}
    </div>

    {#if saveError}
      <div class="error-banner">{saveError}</div>
    {/if}

    {#if editing}
      <textarea
        class="editor-textarea"
        bind:value={editContent}
        spellcheck="false"
      ></textarea>
    {:else}
      <div class="skill-body">
        <pre>{selectedSkill.body}</pre>
      </div>
    {/if}

  {:else if view === 'import'}
    <!-- Import View -->
    <div class="view-header">
      <button class="back-btn" onclick={goBack}>&larr; Back</button>
      <span class="view-title">Import Skill</span>
    </div>

    <div class="import-section">
      <label class="import-label" for="import-url-input">GitHub or skills.sh URL</label>
      <div class="import-input-row">
        <input
          id="import-url-input"
          type="text"
          class="import-input"
          placeholder="https://github.com/user/repo/blob/main/skill.md"
          bind:value={importUrl}
          onkeydown={(e) => { if (e.key === 'Enter') handleFetchPreview(); }}
        />
        <button class="btn btn-fetch" onclick={handleFetchPreview} disabled={importLoading || !importUrl.trim()}>
          {importLoading ? '...' : 'Fetch'}
        </button>
      </div>

      {#if importError}
        <div class="error-banner">{importError}</div>
      {/if}

      {#if importPreview}
        <div class="preview-card">
          <div class="preview-name">{importPreview.skill_name}</div>
          <div class="preview-desc">{importPreview.description}</div>
          <div class="preview-files">
            {#each importPreview.files as file}
              <div class="preview-file">
                <span class="file-name">{file.name}</span>
                <span class="file-size">{Math.round(file.size_bytes / 1024)}KB</span>
              </div>
            {/each}
          </div>
          {#if importPreview.already_exists}
            <div class="warning-banner">Skill already exists. Install will overwrite.</div>
            <button class="btn btn-install" onclick={() => handleInstall(true)} disabled={importLoading}>
              Overwrite & Install
            </button>
          {:else}
            <button class="btn btn-install" onclick={() => handleInstall(false)} disabled={importLoading}>
              Install
            </button>
          {/if}
        </div>
      {/if}
    </div>

  {:else}
    <!-- List View -->
    <div class="view-header">
      <span class="view-title">SKILLS</span>
      <div class="header-actions">
        <button class="action-btn" onclick={openImport}>Import</button>
        <button class="action-btn secondary" onclick={refresh}>Refresh</button>
      </div>
    </div>

    <!-- Agent Selector -->
    {#if agents.length > 0}
      <div class="agent-selector">
        <label class="selector-label" for="skill-agent-select">Agent:</label>
        <select
          id="skill-agent-select"
          class="agent-select"
          value={activeAgentId}
          onchange={(e) => switchAgent(e.currentTarget.value)}
        >
          {#each agents as agent (agent.id)}
            <option value={agent.id}>{agent.name || agent.id}</option>
          {/each}
        </select>
      </div>
    {/if}

    {#if loading}
      <div class="state-msg">Loading...</div>
    {:else if error}
      <div class="state-msg error">{error}</div>
    {:else}
      <!-- Agent Skills -->
      <div class="section">
        <div class="section-label">Assigned ({agentSkills.length})</div>
        {#if agentSkills.length === 0}
          <div class="empty-hint">No skills assigned to this agent</div>
        {:else}
          <div class="skill-list">
            {#each agentSkills as skill (skill.file_name)}
              <div class="skill-row">
                <button class="skill-info" onclick={() => openDetail(skill.file_name)}>
                  <span class="skill-name">{skill.name}</span>
                  <span class="mode-badge {modeBadgeClass(skill.effective_mode)}">{skill.effective_mode}</span>
                  <span class="skill-tokens">{skill.max_tokens}t</span>
                </button>
                <button class="assign-btn unassign" onclick={() => handleUnassign(skill.file_name)} title="Unassign">&minus;</button>
              </div>
            {/each}
          </div>
        {/if}
      </div>

      <!-- Library -->
      <div class="section">
        <div class="section-label">Library ({librarySkills.length})</div>
        {#if librarySkills.length === 0}
          <div class="empty-hint">All skills assigned</div>
        {:else}
          <div class="skill-list">
            {#each librarySkills as skill (skill.file_name)}
              <div class="skill-row">
                <button class="skill-info" onclick={() => openDetail(skill.file_name)}>
                  <span class="skill-name">{skill.name}</span>
                  <span class="mode-badge {modeBadgeClass(skill.effective_mode)}">{skill.effective_mode}</span>
                  <span class="skill-tokens">{skill.max_tokens}t</span>
                </button>
                <button class="assign-btn assign" onclick={() => handleAssign(skill.file_name)} title="Assign to agent">+</button>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .skill-tile {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow-y: auto;
    padding: 12px 14px;
    gap: 10px;
    font-family: var(--mono-family, monospace);
  }

  /* Shared header */
  .view-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .view-title {
    font-size: 0.714rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
  }

  .header-actions {
    display: flex;
    gap: 4px;
  }

  .back-btn {
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    color: var(--text-secondary);
    background: none;
    border: none;
    cursor: pointer;
    padding: 2px 4px;
  }
  .back-btn:hover { color: var(--text-primary); }

  /* Buttons */
  .action-btn {
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    font-weight: 600;
    padding: 3px 10px;
    border: 1px solid var(--agent-accent, #06b6d4);
    border-radius: 4px;
    background: transparent;
    color: var(--agent-accent, #06b6d4);
    cursor: pointer;
  }
  .action-btn:hover {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 12%, transparent);
  }
  .action-btn.secondary {
    border-color: var(--surface-border);
    color: var(--text-muted);
  }
  .action-btn.secondary:hover {
    color: var(--text-primary);
    background: rgba(255, 255, 255, 0.04);
  }

  .btn {
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    font-weight: 600;
    padding: 4px 10px;
    border-radius: 4px;
    border: 1px solid var(--surface-border);
    cursor: pointer;
    background: transparent;
    color: var(--text-secondary);
  }
  .btn-save {
    background: var(--agent-accent, #06b6d4);
    border-color: var(--agent-accent, #06b6d4);
    color: #000;
  }
  .btn-save:disabled { opacity: 0.4; cursor: default; }
  .btn-edit { color: var(--agent-accent, #06b6d4); border-color: var(--agent-accent, #06b6d4); }
  .btn-cancel { color: var(--text-muted); }
  .btn-delete { color: var(--alert-accent, #ef4444); border-color: var(--alert-accent, #ef4444); }
  .btn-delete:hover { background: color-mix(in srgb, var(--alert-accent) 15%, transparent); }
  .btn-fetch { color: var(--agent-accent, #06b6d4); border-color: var(--agent-accent, #06b6d4); }
  .btn-fetch:disabled { opacity: 0.4; cursor: default; }
  .btn-install {
    background: var(--success-accent, #22c55e);
    border-color: var(--success-accent, #22c55e);
    color: #000;
    margin-top: 8px;
    width: 100%;
  }

  /* Agent selector */
  .agent-selector {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 0;
  }

  .selector-label {
    font-size: 0.714rem;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.3px;
    flex-shrink: 0;
  }

  .agent-select {
    flex: 1;
    padding: 4px 8px;
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    color: var(--text-primary);
    background: rgba(0, 0, 0, 0.25);
    border: 1px solid var(--surface-border);
    border-radius: 4px;
    outline: none;
    cursor: pointer;
  }
  .agent-select:focus { border-color: var(--agent-accent, #06b6d4); }

  /* Sections */
  .section {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .section-label {
    font-size: 0.714rem;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    font-weight: 600;
    padding-bottom: 4px;
    border-bottom: 1px solid var(--surface-border);
  }

  .empty-hint {
    font-size: 0.786rem;
    color: var(--text-muted);
    opacity: 0.6;
    padding: 4px 0;
  }

  /* Skill list */
  .skill-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .skill-row {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .skill-info {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 8px;
    border-radius: 3px;
    cursor: pointer;
    min-width: 0;
    background: none;
    border: none;
    font-family: inherit;
    text-align: left;
  }
  .skill-info:hover { background: rgba(255, 255, 255, 0.04); }

  .skill-name {
    font-size: 0.857rem;
    font-weight: 600;
    color: var(--text-primary);
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .skill-tokens {
    font-size: 0.643rem;
    color: var(--text-muted);
    flex-shrink: 0;
  }

  /* Mode badges */
  .mode-badge {
    font-size: 0.643rem;
    font-weight: 700;
    text-transform: uppercase;
    padding: 1px 5px;
    border-radius: 3px;
    flex-shrink: 0;
    letter-spacing: 0.3px;
  }
  .badge-inject {
    color: var(--success-accent, #22c55e);
    background: color-mix(in srgb, var(--success-accent, #22c55e) 12%, transparent);
  }
  .badge-reference {
    color: var(--infra-accent, #6366f1);
    background: color-mix(in srgb, var(--infra-accent, #6366f1) 12%, transparent);
  }
  .badge-auto {
    color: var(--text-muted);
    background: rgba(255, 255, 255, 0.05);
  }

  /* Assign buttons */
  .assign-btn {
    width: 24px;
    height: 24px;
    border: 1px solid var(--surface-border);
    border-radius: 3px;
    background: transparent;
    font-size: 1rem;
    font-weight: 700;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    line-height: 1;
  }
  .assign-btn.assign {
    color: var(--agent-accent, #06b6d4);
    border-color: var(--agent-accent, #06b6d4);
  }
  .assign-btn.assign:hover {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 15%, transparent);
  }
  .assign-btn.unassign {
    color: var(--alert-accent, #ef4444);
    border-color: transparent;
  }
  .assign-btn.unassign:hover {
    background: color-mix(in srgb, var(--alert-accent, #ef4444) 15%, transparent);
  }

  /* Detail view */
  .detail-meta {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 8px 0;
    border-bottom: 1px solid var(--surface-border);
  }

  .meta-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.786rem;
  }

  .meta-label {
    color: var(--text-muted);
    min-width: 50px;
  }

  .meta-value {
    color: var(--text-secondary);
  }

  .meta-hint {
    font-size: 0.643rem;
    color: var(--text-muted);
    opacity: 0.6;
  }

  .skill-body {
    flex: 1;
    overflow-y: auto;
    padding: 8px 0;
  }

  .skill-body pre {
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    color: var(--text-secondary);
    white-space: pre-wrap;
    word-break: break-word;
    margin: 0;
    line-height: 1.5;
  }

  .editor-textarea {
    flex: 1;
    min-height: 200px;
    padding: 10px;
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    color: var(--text-primary);
    background: rgba(0, 0, 0, 0.25);
    border: 1px solid var(--surface-border);
    border-radius: 4px;
    outline: none;
    resize: vertical;
    line-height: 1.5;
    tab-size: 2;
  }
  .editor-textarea:focus { border-color: var(--agent-accent, #06b6d4); }

  /* Error / warning banners */
  .error-banner {
    font-size: 0.786rem;
    color: var(--alert-accent, #ef4444);
    padding: 4px 8px;
    background: color-mix(in srgb, var(--alert-accent, #ef4444) 10%, transparent);
    border-radius: 3px;
  }

  .warning-banner {
    font-size: 0.786rem;
    color: var(--warning-accent, #f59e0b);
    padding: 4px 8px;
    background: color-mix(in srgb, var(--warning-accent, #f59e0b) 10%, transparent);
    border-radius: 3px;
    margin-top: 6px;
  }

  /* Import view */
  .import-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .import-label {
    font-size: 0.714rem;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.3px;
  }

  .import-input-row {
    display: flex;
    gap: 4px;
  }

  .import-input {
    flex: 1;
    padding: 6px 8px;
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    color: var(--text-primary);
    background: rgba(0, 0, 0, 0.25);
    border: 1px solid var(--surface-border);
    border-radius: 4px;
    outline: none;
  }
  .import-input:focus { border-color: var(--agent-accent, #06b6d4); }

  .preview-card {
    border: 1px solid var(--surface-border);
    border-radius: 6px;
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .preview-name {
    font-size: 0.929rem;
    font-weight: 700;
    color: var(--text-primary);
  }

  .preview-desc {
    font-size: 0.786rem;
    color: var(--text-secondary);
  }

  .preview-files {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 4px 0;
  }

  .preview-file {
    display: flex;
    justify-content: space-between;
    font-size: 0.714rem;
    color: var(--text-muted);
    padding: 2px 6px;
    background: rgba(0, 0, 0, 0.15);
    border-radius: 2px;
  }

  .file-name { font-weight: 600; }
  .file-size { opacity: 0.7; }

  /* State messages */
  .state-msg {
    font-size: 0.857rem;
    color: var(--text-muted);
    text-align: center;
    padding: 16px 0;
  }
  .state-msg.error { color: var(--alert-accent, #ef4444); }
</style>
