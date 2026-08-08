<script lang="ts">
  import { onMount } from 'svelte';
  import { listAgents, listDirectory, readFilePreview, sendMessage } from '$lib/tauri/commands';
  import { getAgents } from '$lib/stores/agents.svelte';
  import type { FolderEntry, FilePreview } from '$lib/tauri/commands';
  import { FILE_PREVIEW_LINES } from '$lib/constants';

  interface Props {
    agentId: string;
  }

  let { agentId }: Props = $props();

  let currentPath = $state<string | null>(null);
  let workingDirectory = $state<string | null>(null);
  let entries = $state<FolderEntry[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let showHidden = $state(false);

  // Preview state
  let preview = $state<FilePreview | null>(null);
  let previewLoading = $state(false);
  let previewError = $state<string | null>(null);
  let selectedPath = $state<string | null>(null);

  let agentWorking = $derived(getAgents().find(a => a.id === agentId)?.status === 'Working');

  async function loadDirectory(path: string) {
    loading = true;
    error = null;
    preview = null;
    selectedPath = null;
    try {
      entries = await listDirectory(path, showHidden);
      currentPath = path;
    } catch (err) {
      error = String(err);
      entries = [];
    } finally {
      loading = false;
    }
  }

  function navigateUp() {
    if (!currentPath) return;
    const parent = currentPath.replace(/\/[^/]+\/?$/, '') || '/';
    loadDirectory(parent);
  }

  function navigateInto(entry: FolderEntry) {
    if (entry.is_dir) {
      loadDirectory(entry.path);
    }
  }

  function goHome() {
    if (workingDirectory) loadDirectory(workingDirectory);
  }

  async function handleFileClick(entry: FolderEntry) {
    if (entry.is_dir) return;
    if (selectedPath === entry.path) {
      // Toggle off
      preview = null;
      selectedPath = null;
      return;
    }
    selectedPath = entry.path;
    previewLoading = true;
    previewError = null;
    try {
      preview = await readFilePreview(entry.path, FILE_PREVIEW_LINES);
    } catch (err) {
      previewError = String(err);
      preview = null;
    } finally {
      previewLoading = false;
    }
  }

  async function handleSendToAgent() {
    if (!preview || agentWorking) return;
    try {
      await sendMessage(agentId, `Please read and analyze the file at: ${preview.path}`);
    } catch (err) {
      previewError = String(err);
    }
  }

  function closePreview() {
    preview = null;
    selectedPath = null;
    previewError = null;
  }

  onMount(async () => {
    try {
      const agents = await listAgents();
      const agent = agents.find(a => a.id === agentId);
      if (agent) {
        workingDirectory = agent.working_directory;
        await loadDirectory(agent.working_directory);
      }
    } catch (err) {
      error = String(err);
      loading = false;
    }
  });

  function fileIcon(entry: FolderEntry): string {
    if (entry.is_dir) return '\uD83D\uDCC1';
    const name = entry.name.toLowerCase();
    if (name.endsWith('.rs')) return '\uD83E\uDD80';
    if (name.endsWith('.ts') || name.endsWith('.js')) return '\uD83D\uDCDC';
    if (name.endsWith('.svelte')) return '\uD83D\uDFE0';
    if (name.endsWith('.toml') || name.endsWith('.json') || name.endsWith('.yaml') || name.endsWith('.yml')) return '\u2699\uFE0F';
    if (name.endsWith('.md') || name.endsWith('.txt')) return '\uD83D\uDCDD';
    return '\uD83D\uDCC4';
  }

  function formatSize(bytes: number): string {
    if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
    if (bytes >= 1_000) return `${(bytes / 1_000).toFixed(1)} KB`;
    return `${bytes} B`;
  }
</script>

<div class="file-browser-tile">
  {#if loading}
    <div class="state-msg">Loading...</div>
  {:else if error}
    <div class="state-msg error">{error}</div>
  {:else}
    <!-- Path header with navigation -->
    <div class="path-header">
      <div class="path-nav">
        <button class="nav-btn" onclick={goHome} title="Agent working directory" disabled={currentPath === workingDirectory}>&#8962;</button>
        <button class="nav-btn" onclick={navigateUp} title="Go up" disabled={currentPath === '/'}>&#8593;</button>
        <label class="hidden-toggle">
          <input type="checkbox" bind:checked={showHidden} onchange={() => { if (currentPath) loadDirectory(currentPath); }} />
          <span class="toggle-label">hidden</span>
        </label>
      </div>
      <span class="path-value" title={currentPath ?? ''}>
        {currentPath ?? '(unknown)'} ({entries.length} items)
      </span>
    </div>

    <!-- File list -->
    <div class="file-list">
      {#if entries.length === 0}
        <div class="state-msg">Directory is empty.</div>
      {:else}
        {#each entries as entry (entry.path)}
          <div
            class="file-entry"
            class:is-dir={entry.is_dir}
            class:selected={selectedPath === entry.path}
            onclick={() => handleFileClick(entry)}
            ondblclick={() => navigateInto(entry)}
            role="button"
            tabindex="0"
            onkeydown={(e) => { if (e.key === 'Enter') { entry.is_dir ? navigateInto(entry) : handleFileClick(entry); } }}
          >
            <span class="file-icon">{fileIcon(entry)}</span>
            <span class="file-name">{entry.name}</span>
            {#if entry.agent_dots.length > 0}
              <span class="agent-dots">
                {#each entry.agent_dots as dot}
                  <span class="dot" style="background: rgb({dot.r}, {dot.g}, {dot.b})"></span>
                {/each}
              </span>
            {/if}
          </div>
        {/each}
      {/if}
    </div>

    <!-- Preview pane -->
    {#if previewLoading}
      <div class="preview-pane">
        <div class="state-msg">Loading preview...</div>
      </div>
    {:else if previewError}
      <div class="preview-pane">
        <div class="preview-header">
          <span class="preview-title">ERROR</span>
          <button class="preview-close" onclick={closePreview}>X</button>
        </div>
        <div class="state-msg error">{previewError}</div>
      </div>
    {:else if preview}
      <div class="preview-pane">
        <div class="preview-header">
          <span class="preview-title">PREVIEW: {preview.name} ({formatSize(preview.size_bytes)})</span>
          <div class="preview-actions">
            <button
              class="preview-btn send"
              onclick={handleSendToAgent}
              disabled={agentWorking}
              title={agentWorking ? 'Agent is working' : 'Send file path to agent'}
            >Send</button>
            <button class="preview-btn close" onclick={closePreview}>X</button>
          </div>
        </div>
        <div class="preview-content">
          {#if preview.is_image && preview.base64_content}
            <img class="preview-image" src="data:{preview.media_type};base64,{preview.base64_content}" alt={preview.name} />
          {:else if preview.is_binary}
            <div class="binary-notice">Binary file ({formatSize(preview.size_bytes)})</div>
          {:else}
            <pre class="preview-text">{#each preview.content.split('\n') as line, i}<span class="line-num">{String(i + 1).padStart(3, ' ')}</span>  {line}
{/each}{#if preview.line_count > FILE_PREVIEW_LINES}<span class="truncated">... {preview.line_count - FILE_PREVIEW_LINES} more lines</span>{/if}</pre>
          {/if}
        </div>
      </div>
    {/if}
  {/if}
</div>

<style>
  .file-browser-tile {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }

  .state-msg {
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    color: var(--text-muted);
    padding: 16px;
    text-align: center;
  }

  .state-msg.error {
    color: var(--status-error, #ef4444);
  }

  /* Path header */
  .path-header {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px 12px;
    border-bottom: var(--border-width, 1px) solid var(--surface-border);
    flex-shrink: 0;
  }

  .path-nav {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .nav-btn {
    background: none;
    border: var(--border-width, 1px) solid var(--surface-border);
    border-radius: var(--border-radius, 4px);
    color: var(--text-secondary);
    font-size: 0.929rem;
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    padding: 0;
  }

  .nav-btn:disabled {
    opacity: 0.3;
    cursor: default;
  }

  .nav-btn:not(:disabled):hover {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 10%, transparent);
  }

  .hidden-toggle {
    display: flex;
    align-items: center;
    gap: 3px;
    margin-left: auto;
    cursor: pointer;
  }

  .hidden-toggle input {
    width: 12px;
    height: 12px;
    margin: 0;
  }

  .toggle-label {
    font-family: var(--mono-family, monospace);
    font-size: 0.643rem;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .path-value {
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* File list */
  .file-list {
    flex: 1;
    overflow-y: auto;
    padding: 4px 0;
    min-height: 0;
  }

  .file-entry {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 12px;
    cursor: pointer;
    transition: background 100ms;
  }

  .file-entry:hover {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 6%, transparent);
  }

  .file-entry.selected {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 12%, transparent);
  }

  .file-icon {
    font-size: 0.929rem;
    flex-shrink: 0;
    line-height: 1;
  }

  .file-name {
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    color: var(--text-secondary);
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .file-entry.is-dir .file-name {
    color: var(--text-primary);
    font-weight: 600;
  }

  .agent-dots {
    display: flex;
    gap: 3px;
    flex-shrink: 0;
    margin-left: auto;
  }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    display: inline-block;
  }

  /* Preview pane */
  .preview-pane {
    flex-shrink: 0;
    max-height: 50%;
    border-top: var(--border-width, 1px) solid var(--surface-border);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .preview-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 10px;
    border-bottom: var(--border-width, 1px) solid var(--surface-border);
    flex-shrink: 0;
  }

  .preview-title {
    font-family: var(--mono-family, monospace);
    font-size: 0.714rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.3px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .preview-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }

  .preview-btn {
    font-family: var(--mono-family, monospace);
    font-size: 0.714rem;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: 3px;
    border: 1px solid var(--surface-border, #334155);
    background: transparent;
    cursor: pointer;
  }

  .preview-btn.send {
    color: var(--agent-accent, #06b6d4);
    border-color: var(--agent-accent, #06b6d4);
  }

  .preview-btn.send:hover:not(:disabled) {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 12%, transparent);
  }

  .preview-btn.send:disabled {
    opacity: 0.3;
    cursor: default;
  }

  .preview-btn.close {
    color: var(--text-muted, #64748b);
    border-color: transparent;
  }

  .preview-btn.close:hover {
    color: var(--text-primary);
    background: rgba(255, 255, 255, 0.06);
  }

  .preview-content {
    flex: 1;
    overflow-y: auto;
    padding: 6px 0;
  }

  .preview-text {
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    color: var(--text-secondary, #cbd5e1);
    line-height: 1.5;
    margin: 0;
    padding: 0 10px;
    white-space: pre;
    overflow-x: auto;
  }

  .line-num {
    color: var(--text-muted, #475569);
    opacity: 0.5;
    user-select: none;
  }

  .truncated {
    color: var(--text-muted, #64748b);
    font-style: italic;
  }

  .preview-image {
    max-width: 100%;
    max-height: 300px;
    object-fit: contain;
    display: block;
    margin: 8px auto;
  }

  .binary-notice {
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    color: var(--text-muted);
    text-align: center;
    padding: 16px;
  }
</style>
