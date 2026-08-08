<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { listCheckpoints, createCheckpoint, restoreCheckpoint, deleteCheckpoint } from '$lib/tauri/commands';
  import type { CheckpointSummary } from '$lib/tauri/commands';
  import { CHECKPOINT_REFRESH_INTERVAL_MS } from '$lib/constants';

  interface Props {
    agentId: string;
  }

  let { agentId }: Props = $props();

  let checkpoints = $state<CheckpointSummary[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let refreshInterval: ReturnType<typeof setInterval> | null = null;

  // Inline save input
  let showSaveInput = $state(false);
  let saveLabel = $state('');
  let saving = $state(false);

  async function refresh() {
    try {
      checkpoints = await listCheckpoints(agentId);
      error = null;
    } catch (err) {
      error = String(err);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    refresh();
    refreshInterval = setInterval(refresh, CHECKPOINT_REFRESH_INTERVAL_MS);
  });

  onDestroy(() => {
    if (refreshInterval !== null) {
      clearInterval(refreshInterval);
      refreshInterval = null;
    }
  });

  async function handleSave() {
    const label = saveLabel.trim();
    if (!label) return;
    saving = true;
    try {
      await createCheckpoint(agentId, label);
      saveLabel = '';
      showSaveInput = false;
      await refresh();
    } catch (err) {
      error = String(err);
    } finally {
      saving = false;
    }
  }

  async function handleRestore(cp: CheckpointSummary) {
    try {
      await restoreCheckpoint(agentId, cp.id);
      await refresh();
    } catch (err) {
      error = String(err);
    }
  }

  async function handleDelete(cp: CheckpointSummary) {
    try {
      await deleteCheckpoint(agentId, cp.id);
      await refresh();
    } catch (err) {
      error = String(err);
    }
  }

  function handleSaveKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleSave();
    } else if (e.key === 'Escape') {
      showSaveInput = false;
      saveLabel = '';
    }
  }

  function formatTimestamp(iso: string): string {
    try {
      const d = new Date(iso);
      const now = Date.now();
      const diffMs = now - d.getTime();
      if (diffMs < 60_000) return 'just now';
      if (diffMs < 3600_000) return `${Math.floor(diffMs / 60_000)}m ago`;
      if (diffMs < 86400_000) return `${Math.floor(diffMs / 3600_000)}h ago`;
      return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' }) +
        ' ' + d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
    } catch {
      return iso;
    }
  }

  function formatTokens(n: number): string {
    if (n >= 1000) return `~${(n / 1000).toFixed(1)}k`;
    return `~${n}`;
  }
</script>

<div class="checkpoint-tile">
  <!-- Header -->
  <div class="header">
    <span class="title">CHECKPOINTS</span>
    <button class="action-btn" onclick={() => { showSaveInput = !showSaveInput; }}>+ Save</button>
  </div>

  <!-- Inline save input -->
  {#if showSaveInput}
    <div class="save-row">
      <input
        class="save-input"
        type="text"
        placeholder="Checkpoint label..."
        bind:value={saveLabel}
        onkeydown={handleSaveKeydown}
        disabled={saving}
      />
      <button class="save-btn" onclick={handleSave} disabled={saving || !saveLabel.trim()}>
        {saving ? '...' : 'Save'}
      </button>
      <button class="cancel-btn" onclick={() => { showSaveInput = false; saveLabel = ''; }}>X</button>
    </div>
  {/if}

  {#if loading}
    <div class="state-msg">Loading...</div>
  {:else if error}
    <div class="state-msg error">{error}</div>
  {:else if checkpoints.length === 0}
    <div class="state-msg">No checkpoints saved</div>
  {:else}
    <div class="count-label">{checkpoints.length} checkpoint{checkpoints.length === 1 ? '' : 's'}</div>
    <div class="checkpoint-list">
      {#each checkpoints as cp (cp.id)}
        <div class="checkpoint-entry">
          <div class="cp-label">{cp.label}</div>
          <div class="cp-meta">
            <span class="cp-time">{formatTimestamp(cp.created_at)}</span>
            <span class="cp-sep">&middot;</span>
            <span class="cp-msgs">{cp.message_count} msgs</span>
            <span class="cp-sep">&middot;</span>
            <span class="cp-tokens">{formatTokens(cp.context_token_estimate)} tokens</span>
          </div>
          <div class="cp-actions">
            <button class="btn btn-restore" onclick={() => handleRestore(cp)}>Restore</button>
            <button class="btn btn-delete" onclick={() => handleDelete(cp)}>Delete</button>
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .checkpoint-tile {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow-y: auto;
    padding: 12px 14px;
    gap: 10px;
    font-family: var(--mono-family, monospace);
  }

  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .title {
    font-size: 0.714rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
  }

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

  /* Save input row */
  .save-row {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .save-input {
    flex: 1;
    padding: 5px 8px;
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    color: var(--text-primary, #e2e8f0);
    background: rgba(0, 0, 0, 0.25);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 4px;
    outline: none;
  }

  .save-input:focus {
    border-color: var(--agent-accent, #06b6d4);
  }

  .save-btn {
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    font-weight: 600;
    padding: 5px 10px;
    border: 1px solid var(--agent-accent, #06b6d4);
    border-radius: 4px;
    background: var(--agent-accent, #06b6d4);
    color: #000;
    cursor: pointer;
  }

  .save-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .cancel-btn {
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    padding: 5px 6px;
    border: none;
    background: transparent;
    color: var(--text-muted, #64748b);
    cursor: pointer;
    border-radius: 3px;
  }

  .cancel-btn:hover {
    color: var(--text-primary);
    background: rgba(255, 255, 255, 0.06);
  }

  /* State messages */
  .state-msg {
    font-size: 0.857rem;
    color: var(--text-muted);
    text-align: center;
    padding: 16px 0;
  }

  .state-msg.error {
    color: var(--status-error, #ef4444);
  }

  .count-label {
    font-size: 0.786rem;
    color: var(--text-muted);
  }

  /* Checkpoint list */
  .checkpoint-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .checkpoint-entry {
    padding: 8px 10px;
    border: 1px solid var(--surface-border, #334155);
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.02);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .checkpoint-entry:hover {
    background: rgba(255, 255, 255, 0.04);
  }

  .cp-label {
    font-size: 0.929rem;
    font-weight: 600;
    color: var(--text-primary, #e2e8f0);
  }

  .cp-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
  }

  .cp-sep {
    opacity: 0.4;
  }

  .cp-actions {
    display: flex;
    gap: 6px;
    justify-content: flex-end;
    margin-top: 2px;
  }

  .btn {
    font-family: var(--mono-family, monospace);
    font-size: 0.714rem;
    font-weight: 600;
    padding: 3px 8px;
    border-radius: 3px;
    border: 1px solid var(--surface-border, #334155);
    background: transparent;
    cursor: pointer;
    transition: background 100ms, color 100ms;
  }

  .btn-restore {
    color: var(--agent-accent, #06b6d4);
    border-color: var(--agent-accent, #06b6d4);
  }

  .btn-restore:hover {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 15%, transparent);
  }

  .btn-delete {
    color: var(--status-error, #ef4444);
    border-color: transparent;
  }

  .btn-delete:hover {
    background: color-mix(in srgb, var(--status-error, #ef4444) 15%, transparent);
  }
</style>
