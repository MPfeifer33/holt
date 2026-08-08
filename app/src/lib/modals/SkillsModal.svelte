<script lang="ts">
  import { onMount } from 'svelte';
  import ModalShell from './ModalShell.svelte';
  import {
    listSkills, getSkill, createSkill, updateSkill, deleteSkill,
    assignSkillToAgent, unassignSkillFromAgent, getActiveSkills,
    fetchSkillPreview, installImportedSkill,
  } from '$lib/tauri/commands';
  import type { SkillSummary, SkillDetail, ImportPreview } from '$lib/tauri/commands';
  import { getAgents } from '$lib/stores/agents.svelte';

  interface Props { onclose: () => void; }
  let { onclose }: Props = $props();

  // State
  let allSkills = $state<SkillSummary[]>([]);
  let selectedSkill = $state<SkillDetail | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let editing = $state(false);
  let editContent = $state('');
  let saving = $state(false);
  let saveError = $state<string | null>(null);
  let isNewSkill = $state(false);

  // Import state
  let importMode = $state(false);
  let importUrl = $state('');
  let importLoading = $state(false);
  let importPreview = $state<ImportPreview | null>(null);
  let importError = $state<string | null>(null);

  // Agent assignment state
  let agents = $derived(getAgents());
  let agentSkillMap = $state<Record<string, Set<string>>>({});

  // Load data
  onMount(async () => {
    await refresh();
  });

  async function refresh() {
    loading = true;
    error = null;
    try {
      allSkills = await listSkills();

      // Load agent skill assignments
      const map: Record<string, Set<string>> = {};
      for (const agent of getAgents()) {
        try {
          const active = await getActiveSkills(agent.id);
          map[agent.id] = new Set(active.map(a => a.name)); // name = file_name from backend
        } catch {
          map[agent.id] = new Set();
        }
      }
      agentSkillMap = map;
    } catch (err) {
      error = String(err);
    } finally {
      loading = false;
    }
  }

  async function selectSkill(fileName: string) {
    try {
      selectedSkill = await getSkill(fileName);
      editing = false;
      isNewSkill = false;
      editContent = '';
      saveError = null;
    } catch (err) {
      error = String(err);
    }
  }

  function startEditing() {
    if (!selectedSkill) return;
    editContent = selectedSkill.raw_content;
    editing = true;
    isNewSkill = false;
    saveError = null;
  }

  function cancelEditing() {
    editing = false;
    isNewSkill = false;
    editContent = '';
    saveError = null;
  }

  async function handleSave() {
    if (!selectedSkill) return;
    saving = true;
    saveError = null;
    try {
      await updateSkill(selectedSkill.file_name, editContent);
      await refresh();
      await selectSkill(selectedSkill.file_name);
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
      await refresh();
    } catch (err) {
      error = String(err);
    }
  }

  function handleNewSkill() {
    selectedSkill = null;
    isNewSkill = true;
    editContent = `---
name: new-skill
description:
priority: normal
triggers: []
max_tokens: 2048
---

Skill body here.
`;
    editing = true;
    saveError = null;
  }

  async function handleCreateSkill() {
    saving = true;
    saveError = null;
    try {
      const nameMatch = editContent.match(/^name:\s*(.+)$/m);
      const derivedName = nameMatch ? nameMatch[1].trim() : 'new-skill';
      const fileName = derivedName.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'new-skill';
      await createSkill(fileName, editContent);
      await refresh();
      await selectSkill(fileName);
      editing = false;
      isNewSkill = false;
    } catch (err) {
      saveError = String(err);
    } finally {
      saving = false;
    }
  }

  // Agent assignment — uses file_name consistently
  async function toggleAgentSkill(agentId: string, skillFileName: string) {
    const current = agentSkillMap[agentId] ?? new Set();
    try {
      if (current.has(skillFileName)) {
        await unassignSkillFromAgent(agentId, skillFileName);
      } else {
        await assignSkillToAgent(agentId, skillFileName);
      }
      // Refresh assignments
      const active = await getActiveSkills(agentId);
      agentSkillMap = { ...agentSkillMap, [agentId]: new Set(active.map(a => a.name)) };
    } catch (err) {
      error = String(err);
    }
  }

  function isAssigned(agentId: string, skillFileName: string): boolean {
    return agentSkillMap[agentId]?.has(skillFileName) ?? false;
  }

  // Import handlers
  function startImport() {
    importMode = true;
    importUrl = '';
    importPreview = null;
    importError = null;
  }

  function cancelImport() {
    importMode = false;
    importUrl = '';
    importLoading = false;
    importPreview = null;
    importError = null;
  }

  async function handleFetch() {
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

  async function handleInstall() {
    if (!importPreview) return;
    try {
      await installImportedSkill(
        importPreview.skill_name,
        importPreview.files,
        importPreview.source_url,
        importPreview.commit_sha,
        importPreview.already_exists,
      );
      const skillName = importPreview.skill_name;
      await refresh();
      cancelImport();
      await selectSkill(skillName);
    } catch (err) {
      importError = String(err);
    }
  }
</script>

<ModalShell title="Skills" width="700px" {onclose}>
  <div class="skills-layout">
    <!-- LEFT: Skill List -->
    <div class="skill-list-col">
      <div class="list-header">
        <span class="list-title">Library ({allSkills.length})</span>
        <div class="list-actions">
          <button class="btn-new" onclick={startImport}>Import</button>
          <button class="btn-new" onclick={handleNewSkill}>+ New</button>
        </div>
      </div>

      {#if importMode}
        <div class="import-row">
          <input
            class="import-input"
            type="text"
            placeholder="Paste GitHub or skills.sh URL..."
            bind:value={importUrl}
            onkeydown={(e) => { if (e.key === 'Enter') handleFetch(); }}
          />
          <button class="btn-fetch" onclick={handleFetch} disabled={importLoading || !importUrl.trim()}>
            {importLoading ? '...' : 'Fetch'}
          </button>
          <button class="btn-cancel-import" onclick={cancelImport}>&#x2715;</button>
        </div>
        {#if importError && !importPreview}
          <div class="import-error">{importError}</div>
        {/if}
      {/if}

      {#if loading}
        <div class="state-msg">Loading...</div>
      {:else if error}
        <div class="state-msg error">{error}</div>
      {:else}
        <div class="skill-list">
          {#each allSkills as skill (skill.file_name)}
            <button
              class="skill-item"
              class:selected={selectedSkill?.file_name === skill.file_name}
              onclick={() => selectSkill(skill.file_name)}
            >
              <div class="skill-item-name">{skill.name}</div>
              <div class="skill-item-meta">
                <span>{skill.priority}</span>
                <span>{skill.trigger_count} trig</span>
                {#if skill.is_directory}
                  <span>{skill.file_count} files</span>
                {/if}
              </div>
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <!-- RIGHT: Detail / Editor / Import Preview -->
    <div class="skill-detail-col">
      {#if importPreview}
        <!-- Import Preview -->
        <div class="import-preview">
          <div class="import-warning">
            &#x26A0; Skills run with full agent permissions. Review all content before installing.
          </div>

          <div class="import-meta">
            <div class="detail-title">{importPreview.skill_name}</div>
            <div class="detail-file">{importPreview.description}</div>
            <div class="import-source">
              Source: {importPreview.source_url}
              <span class="commit-sha">({importPreview.commit_sha})</span>
            </div>
          </div>

          <div class="import-files">
            {#each importPreview.files as file (file.name)}
              <details open>
                <summary class="file-summary">
                  {file.name}
                  <span class="file-size">({(file.size_bytes / 1024).toFixed(1)} KB)</span>
                </summary>
                <pre class="file-content">{file.content}</pre>
              </details>
            {/each}
          </div>

          {#if importError}
            <div class="import-error" style="padding: 8px 16px;">{importError}</div>
          {/if}

          <div class="import-actions">
            <button class="btn-cancel-edit" onclick={cancelImport}>Cancel</button>
            <button
              class="btn-save"
              class:overwrite={importPreview.already_exists}
              onclick={handleInstall}
            >
              {importPreview.already_exists ? 'Overwrite' : 'Install'}
            </button>
          </div>
        </div>

      {:else if editing}
        <!-- Editor -->
        <div class="editor-header">
          <span class="detail-title">{isNewSkill ? 'New Skill' : 'Edit Skill'}</span>
          <div class="editor-actions">
            <button class="btn-cancel-edit" onclick={cancelEditing}>Cancel</button>
            <button
              class="btn-save"
              onclick={isNewSkill ? handleCreateSkill : handleSave}
              disabled={saving}
            >
              {saving ? 'Saving...' : 'Save'}
            </button>
          </div>
        </div>
        {#if saveError}
          <div class="save-error">{saveError}</div>
        {/if}
        <textarea
          class="editor-textarea"
          bind:value={editContent}
          spellcheck="false"
        ></textarea>

      {:else if selectedSkill}
        <!-- Detail View -->
        <div class="detail-header">
          <div>
            <span class="detail-title">{selectedSkill.name}</span>
            <span class="detail-file">{selectedSkill.file_name}{selectedSkill.is_directory ? '/' : '.md'}</span>
          </div>
          <div class="detail-actions">
            <button class="btn-edit" onclick={startEditing}>Edit</button>
            <button class="btn-delete" onclick={handleDelete}>Delete</button>
          </div>
        </div>

        <div class="detail-meta">
          <span>Priority: {selectedSkill.priority}</span>
          <span>Max tokens: {selectedSkill.max_tokens}</span>
          {#if selectedSkill.is_directory && selectedSkill.files.length > 0}
            <span>Files: {selectedSkill.files.join(', ')}</span>
          {/if}
        </div>

        <div class="detail-body">{selectedSkill.body}</div>

        <!-- Agent Assignment -->
        <div class="assignment-section">
          <span class="assignment-title">Agent Assignment</span>
          <div class="agent-list">
            {#each agents as agent (agent.id)}
              <button
                class="agent-toggle"
                class:assigned={isAssigned(agent.id, selectedSkill?.file_name ?? '')}
                onclick={() => { if (selectedSkill) toggleAgentSkill(agent.id, selectedSkill.file_name); }}
              >
                <span
                  class="agent-dot"
                  style="background: rgb({agent.color.r}, {agent.color.g}, {agent.color.b});"
                ></span>
                {agent.name}
                <span class="toggle-indicator">
                  {isAssigned(agent.id, selectedSkill?.file_name ?? '') ? '✓' : '+'}
                </span>
              </button>
            {/each}
          </div>
        </div>

      {:else}
        <div class="empty-detail">
          <p>Select a skill to view details</p>
          <p class="muted">or click + New to create one</p>
        </div>
      {/if}
    </div>
  </div>
</ModalShell>

<style>
  .skills-layout {
    display: flex;
    height: 480px;
  }

  /* Left column */
  .skill-list-col {
    flex: 0 0 240px;
    border-right: var(--border-width) solid var(--surface-border);
    display: flex;
    flex-direction: column;
  }

  .list-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 14px;
    border-bottom: var(--border-width) solid var(--surface-border);
  }

  .list-title {
    font-size: 0.786rem;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: var(--text-transform, none);
    letter-spacing: 0.05em;
  }

  .btn-new {
    font-size: 0.786rem;
    font-weight: 600;
    padding: 3px 10px;
    border: var(--border-width) solid var(--agent-accent);
    border-radius: var(--border-radius);
    background: transparent;
    color: var(--agent-accent);
    cursor: pointer;
    font-family: var(--font-family);
  }

  .btn-new:hover {
    background: color-mix(in srgb, var(--agent-accent) 12%, transparent);
  }

  .skill-list {
    flex: 1;
    overflow-y: auto;
    padding: 4px 0;
  }

  .skill-item {
    display: block;
    width: 100%;
    text-align: left;
    padding: 8px 14px;
    background: none;
    border: none;
    cursor: pointer;
    color: var(--text-primary);
    font-family: var(--font-family);
    border-left: 2px solid transparent;
    transition: background 0.1s;
  }

  .skill-item:hover {
    background: color-mix(in srgb, var(--surface-border) 30%, transparent);
  }

  .skill-item.selected {
    background: color-mix(in srgb, var(--agent-accent) 8%, transparent);
    border-left-color: var(--agent-accent);
  }

  .skill-item-name {
    font-size: 0.929rem;
    font-weight: 600;
  }

  .skill-item-meta {
    font-size: 0.714rem;
    color: var(--text-muted);
    display: flex;
    gap: 8px;
    margin-top: 2px;
  }

  /* Right column */
  .skill-detail-col {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    overflow-y: auto;
    padding: 16px;
  }

  .detail-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    margin-bottom: 12px;
  }

  .detail-title {
    font-size: 1.071rem;
    font-weight: 700;
    color: var(--text-primary);
  }

  .detail-file {
    display: block;
    font-size: 0.786rem;
    color: var(--text-muted);
    font-family: var(--mono-family);
    margin-top: 2px;
  }

  .detail-actions {
    display: flex;
    gap: 6px;
  }

  .btn-edit, .btn-cancel-edit {
    font-size: 0.786rem;
    padding: 4px 12px;
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
    font-family: var(--font-family);
  }

  .btn-edit:hover, .btn-cancel-edit:hover {
    color: var(--text-primary);
    border-color: var(--text-muted);
  }

  .btn-delete {
    font-size: 0.786rem;
    padding: 4px 12px;
    border: var(--border-width) solid var(--alert-accent);
    border-radius: var(--border-radius);
    background: transparent;
    color: var(--alert-accent);
    cursor: pointer;
    font-family: var(--font-family);
  }

  .btn-delete:hover {
    background: color-mix(in srgb, var(--alert-accent) 12%, transparent);
  }

  .btn-save {
    font-size: 0.786rem;
    font-weight: 600;
    padding: 4px 12px;
    border: none;
    border-radius: var(--border-radius);
    background: var(--agent-accent);
    color: #fff;
    cursor: pointer;
    font-family: var(--font-family);
  }

  .btn-save:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .detail-meta {
    display: flex;
    gap: 12px;
    font-size: 0.786rem;
    color: var(--text-secondary);
    margin-bottom: 12px;
    padding-bottom: 12px;
    border-bottom: var(--border-width) solid var(--surface-border);
  }

  .detail-body {
    font-size: 0.929rem;
    color: var(--text-primary);
    line-height: 1.6;
    white-space: pre-wrap;
    flex: 1;
    overflow-y: auto;
    margin-bottom: 16px;
  }

  /* Editor */
  .editor-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 8px;
  }

  .editor-actions {
    display: flex;
    gap: 6px;
  }

  .save-error {
    font-size: 0.786rem;
    color: var(--alert-accent);
    padding: 6px 8px;
    background: color-mix(in srgb, var(--alert-accent) 8%, transparent);
    border-radius: var(--border-radius);
    margin-bottom: 8px;
  }

  .editor-textarea {
    flex: 1;
    min-height: 300px;
    padding: 10px;
    font-family: var(--mono-family);
    font-size: 0.857rem;
    color: var(--text-primary);
    background: rgba(0, 0, 0, 0.2);
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    outline: none;
    resize: none;
    line-height: 1.5;
  }

  .editor-textarea:focus {
    border-color: var(--agent-accent);
  }

  /* Agent Assignment */
  .assignment-section {
    border-top: var(--border-width) solid var(--surface-border);
    padding-top: 12px;
  }

  .assignment-title {
    font-size: 0.786rem;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: var(--text-transform, none);
    letter-spacing: 0.05em;
  }

  .agent-list {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 8px;
  }

  .agent-toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border-radius: 14px;
    border: var(--border-width) solid var(--surface-border);
    background: transparent;
    color: var(--text-secondary);
    font-size: 0.786rem;
    font-family: var(--font-family);
    cursor: pointer;
    transition: all 0.1s;
  }

  .agent-toggle:hover {
    border-color: var(--text-muted);
    color: var(--text-primary);
  }

  .agent-toggle.assigned {
    border-color: var(--success-accent);
    background: color-mix(in srgb, var(--success-accent) 8%, transparent);
    color: var(--text-primary);
  }

  .agent-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .toggle-indicator {
    font-weight: 700;
    font-size: 0.857rem;
  }

  .agent-toggle.assigned .toggle-indicator {
    color: var(--success-accent);
  }

  /* Empty state */
  .empty-detail {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
    font-size: 0.929rem;
  }

  .empty-detail .muted {
    font-size: 0.786rem;
    opacity: 0.6;
    margin-top: 4px;
  }

  .state-msg {
    font-size: 0.857rem;
    color: var(--text-muted);
    text-align: center;
    padding: 24px 0;
  }

  .state-msg.error {
    color: var(--alert-accent);
  }

  /* Import UI */
  .list-actions {
    display: flex;
    gap: 4px;
  }

  .import-row {
    display: flex;
    gap: 4px;
    padding: 8px 14px;
    border-bottom: var(--border-width) solid var(--surface-border);
  }

  .import-input {
    flex: 1;
    background: var(--card-bg);
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    color: var(--text-primary);
    font-family: var(--font-family);
    font-size: 0.786rem;
    padding: 4px 8px;
    outline: none;
  }

  .import-input:focus {
    border-color: var(--agent-accent);
  }

  .btn-fetch {
    font-size: 0.786rem;
    font-weight: 600;
    padding: 4px 10px;
    border: var(--border-width) solid var(--agent-accent);
    border-radius: var(--border-radius);
    background: transparent;
    color: var(--agent-accent);
    cursor: pointer;
    font-family: var(--font-family);
  }

  .btn-fetch:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .btn-cancel-import {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 1rem;
  }

  .import-error {
    font-size: 0.786rem;
    color: var(--alert-accent);
    padding: 4px 14px;
  }

  /* Import Preview */
  .import-preview {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .import-warning {
    padding: 10px 16px;
    background: color-mix(in srgb, var(--warning-accent) 10%, transparent);
    border-bottom: var(--border-width) solid color-mix(in srgb, var(--warning-accent) 30%, transparent);
    color: var(--warning-accent);
    font-size: 0.857rem;
    font-weight: 600;
    flex-shrink: 0;
  }

  .import-meta {
    padding: 12px 16px;
    border-bottom: var(--border-width) solid var(--surface-border);
    flex-shrink: 0;
  }

  .import-source {
    font-size: 0.786rem;
    color: var(--text-muted);
    margin-top: 6px;
    word-break: break-all;
  }

  .commit-sha {
    font-family: var(--mono-family);
    font-size: 0.714rem;
  }

  .import-files {
    flex: 1;
    overflow-y: auto;
    padding: 8px 16px;
  }

  .file-summary {
    font-size: 0.857rem;
    font-weight: 600;
    color: var(--text-primary);
    cursor: pointer;
    padding: 6px 0;
    border-bottom: var(--border-width) solid color-mix(in srgb, var(--surface-border) 40%, transparent);
  }

  .file-size {
    font-weight: 400;
    color: var(--text-muted);
    font-size: 0.714rem;
  }

  .file-content {
    font-family: var(--mono-family);
    font-size: 0.786rem;
    color: var(--text-secondary);
    line-height: 1.5;
    padding: 8px;
    background: rgba(0, 0, 0, 0.15);
    border-radius: var(--border-radius);
    overflow-x: auto;
    white-space: pre-wrap;
    margin: 4px 0 12px;
  }

  .import-actions {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
    padding: 12px 16px;
    border-top: var(--border-width) solid var(--surface-border);
    flex-shrink: 0;
  }

  .btn-save.overwrite {
    background: var(--alert-accent);
  }
</style>
