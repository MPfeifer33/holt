<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import {
    listTriggers,
    createTrigger,
    deleteTrigger,
    toggleTrigger,
    fireTrigger,
    listAgents,
    type TriggerJobResponse,
    type AgentSummary,
  } from '$lib/tauri/commands';

  let jobs = $state<TriggerJobResponse[]>([]);
  let agents = $state<AgentSummary[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let actionInProgress = $state<Record<string, boolean>>({});
  let showCreate = $state(false);

  // Create form fields
  let newName = $state('');
  let newAgentId = $state('');
  let newCron = $state('');
  let newMessage = $state('');
  let creating = $state(false);
  let createError = $state<string | null>(null);

  async function loadJobs() {
    loading = true;
    error = null;
    try {
      const [jobList, agentList] = await Promise.all([
        listTriggers(),
        listAgents(),
      ]);
      jobs = jobList;
      agents = agentList;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function agentName(agentId: string): string {
    const agent = agents.find(a => a.id === agentId);
    return agent ? agent.name : agentId.slice(0, 8);
  }

  function relativeTime(iso: string | null): string {
    if (!iso) return '\u2014';
    const dt = new Date(iso);
    const now = Date.now();
    const diff = now - dt.getTime();
    if (diff < 0) {
      // future
      const absDiff = Math.abs(diff);
      if (absDiff < 60_000) return `in ${Math.round(absDiff / 1000)}s`;
      if (absDiff < 3_600_000) return `in ${Math.round(absDiff / 60_000)}m`;
      if (absDiff < 86_400_000) return `in ${Math.round(absDiff / 3_600_000)}h`;
      return `in ${Math.round(absDiff / 86_400_000)}d`;
    }
    if (diff < 60_000) return `${Math.round(diff / 1000)}s ago`;
    if (diff < 3_600_000) return `${Math.round(diff / 60_000)}m ago`;
    if (diff < 86_400_000) return `${Math.round(diff / 3_600_000)}h ago`;
    return `${Math.round(diff / 86_400_000)}d ago`;
  }

  async function handleCreate() {
    if (!newName.trim() || !newAgentId || !newCron.trim() || !newMessage.trim()) return;
    creating = true;
    createError = null;
    try {
      await createTrigger(newName.trim(), newAgentId, newCron.trim(), newMessage.trim());
      newName = '';
      newCron = '';
      newMessage = '';
      showCreate = false;
      await loadJobs();
    } catch (e) {
      createError = e instanceof Error ? e.message : String(e);
    } finally {
      creating = false;
    }
  }

  async function handleToggle(job: TriggerJobResponse) {
    actionInProgress[job.id] = true;
    try {
      await toggleTrigger(job.id);
      await loadJobs();
    } catch {
      await loadJobs();
    } finally {
      actionInProgress[job.id] = false;
    }
  }

  async function handleFire(jobId: string) {
    actionInProgress[jobId] = true;
    try {
      await fireTrigger(jobId);
      await loadJobs();
    } catch {
      await loadJobs();
    } finally {
      actionInProgress[jobId] = false;
    }
  }

  async function handleDelete(jobId: string) {
    actionInProgress[jobId] = true;
    try {
      await deleteTrigger(jobId);
      await loadJobs();
    } catch {
      await loadJobs();
    } finally {
      actionInProgress[jobId] = false;
    }
  }

  // Split jobs into schedules and watches
  let schedules = $derived(jobs.filter(j => j.job_type !== 'file_watch'));
  let watches = $derived(jobs.filter(j => j.job_type === 'file_watch'));

  // Load on mount + listen for trigger events
  onMount(() => {
    void loadJobs();
    const unlistenFired = listen('trigger-fired', () => loadJobs());
    const unlistenSkipped = listen('trigger-skipped', () => loadJobs());
    return () => {
      void unlistenFired.then(fn => fn());
      void unlistenSkipped.then(fn => fn());
    };
  });
</script>

<div class="cron-tab">
  <p class="subtitle">Scheduled jobs that send messages to agents on a cron schedule</p>

  <!-- Create toggle -->
  <button class="btn-create" onclick={() => { showCreate = !showCreate; }}>
    {showCreate ? '\u2715 Cancel' : '+ New Schedule'}
  </button>

  <!-- Create form -->
  {#if showCreate}
    <div class="create-form">
      <div class="form-row">
        <label class="form-label" for="cron-name">Name</label>
        <input
          id="cron-name"
          class="form-input"
          type="text"
          placeholder="Daily summary"
          bind:value={newName}
        />
      </div>
      <div class="form-row">
        <label class="form-label" for="cron-agent">Agent</label>
        <select id="cron-agent" class="form-input" bind:value={newAgentId}>
          <option value="" disabled>Select agent...</option>
          {#each agents as agent (agent.id)}
            <option value={agent.id}>{agent.name}</option>
          {/each}
        </select>
      </div>
      <div class="form-row">
        <label class="form-label" for="cron-expr">Schedule</label>
        <input
          id="cron-expr"
          class="form-input mono"
          type="text"
          placeholder="0 9 * * *"
          bind:value={newCron}
        />
      </div>
      <div class="form-row">
        <label class="form-label" for="cron-msg">Message</label>
        <textarea
          id="cron-msg"
          class="form-input form-textarea"
          placeholder="Generate the daily report..."
          bind:value={newMessage}
          rows="3"
        ></textarea>
      </div>
      {#if createError}
        <p class="error-text">{createError}</p>
      {/if}
      <button
        class="btn-primary"
        onclick={handleCreate}
        disabled={creating || !newName.trim() || !newAgentId || !newCron.trim() || !newMessage.trim()}
      >
        {creating ? 'Creating...' : 'Create Schedule'}
      </button>
    </div>
  {/if}

  <!-- Job list -->
  {#if loading}
    <p class="hint">Loading schedules...</p>
  {:else if error}
    <p class="error-text">{error}</p>
    <button class="btn-ghost" onclick={loadJobs}>Retry</button>
  {:else if schedules.length === 0}
    <div class="empty-state">
      <p class="hint">No scheduled jobs yet.</p>
      <p class="hint">Create one to send messages to agents on a timer.</p>
    </div>
  {:else}
    <div class="job-list">
      {#each schedules as job (job.id)}
        <div class="job-row" class:disabled={!job.enabled}>
          <div class="job-header">
            <div class="job-status-dot" class:active={job.enabled}></div>
            <div class="job-info">
              <span class="job-name">{job.name}</span>
              <span class="job-agent">{agentName(job.agent_id)}</span>
            </div>
            <div class="job-controls">
              <button
                class="toggle-switch"
                class:on={job.enabled}
                onclick={() => handleToggle(job)}
                disabled={actionInProgress[job.id]}
                aria-label={job.enabled ? 'Disable schedule' : 'Enable schedule'}
              ></button>
            </div>
          </div>

          <div class="job-details">
            <div class="detail-grid">
              <div class="detail-cell">
                <span class="detail-label">Schedule</span>
                <span class="detail-value mono">{job.cron_expression}</span>
              </div>
              <div class="detail-cell">
                <span class="detail-label">Next Fire</span>
                <span class="detail-value">{relativeTime(job.next_fire)}</span>
              </div>
              <div class="detail-cell">
                <span class="detail-label">Last Fired</span>
                <span class="detail-value">{relativeTime(job.last_fired)}</span>
              </div>
              <div class="detail-cell">
                <span class="detail-label">Fires / Skips</span>
                <span class="detail-value">{job.fire_count} / {job.skip_count}</span>
              </div>
            </div>
            <div class="job-message">
              <span class="detail-label">Message</span>
              <span class="detail-value msg-text">{job.message}</span>
            </div>
            <div class="job-actions">
              <button
                class="btn-ghost btn-sm"
                onclick={() => handleFire(job.id)}
                disabled={actionInProgress[job.id]}
                title="Fire now"
              >
                &#x25B6; Fire Now
              </button>
              <button
                class="btn-ghost btn-sm btn-danger"
                onclick={() => handleDelete(job.id)}
                disabled={actionInProgress[job.id]}
                title="Delete schedule"
              >
                &#x2715; Delete
              </button>
            </div>
          </div>
        </div>
      {/each}
    </div>
  {/if}

  <!-- Watches Section -->
  <div class="section-divider"></div>
  <h3 class="section-title">File Watches</h3>
  <p class="subtitle">Agents are notified when watched files change. Use watch_path tool to add custom watches.</p>

  {#if watches.length > 0}
    <div class="watch-list">
      {#each watches as watch (watch.id)}
        <div class="watch-item">
          <div class="watch-header">
            <label class="toggle">
              <input type="checkbox" checked={watch.enabled}
                onchange={() => handleToggle(watch)} />
              <span class="slider"></span>
            </label>
            <div class="watch-info">
              <span class="watch-path">{watch.watch_config?.path ?? 'unknown'}</span>
              {#if watch.watch_config?.glob_filter}
                <span class="watch-glob">({watch.watch_config.glob_filter})</span>
              {/if}
              {#if watch.internal}
                <span class="default-label">default</span>
              {/if}
            </div>
          </div>
          <div class="watch-details">
            <span>{agentName(watch.agent_id)}</span>
            <span>Events: {watch.watch_config?.events?.join(', ') ?? 'all'}</span>
            <span>Fires: {watch.fire_count}</span>
            {#if watch.last_fired}
              <span>Last: {relativeTime(watch.last_fired)}</span>
            {/if}
          </div>
          {#if !watch.internal}
            <button class="btn-ghost btn-sm btn-danger" onclick={() => handleDelete(watch.id)}
              disabled={actionInProgress[watch.id]}>
              &#x2715; Remove
            </button>
          {/if}
        </div>
      {/each}
    </div>
  {:else}
    <p class="no-watches">No file watches configured. Agents can add watches with the watch_path tool.</p>
  {/if}
</div>

<style>
  .cron-tab {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .subtitle {
    font-size: 0.786rem;
    color: var(--text-muted, #64748b);
    margin-bottom: 8px;
  }

  .hint {
    font-size: 0.786rem;
    color: var(--text-muted, #64748b);
    margin: 0;
    line-height: 1.4;
  }

  .error-text {
    font-size: 0.786rem;
    color: var(--alert-accent, #ef4444);
    margin: 0 0 8px 0;
  }

  .empty-state {
    text-align: center;
    padding: 40px 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  /* Create button */
  .btn-create {
    background: transparent;
    border: 1px dashed var(--surface-border, #334155);
    color: var(--text-muted, #64748b);
    padding: 8px 12px;
    border-radius: 6px;
    font-size: 0.786rem;
    cursor: pointer;
    transition: border-color 0.15s, color 0.15s;
  }

  .btn-create:hover {
    border-color: var(--agent-accent, #06b6d4);
    color: var(--agent-accent, #06b6d4);
  }

  /* Create form */
  .create-form {
    background: var(--card-bg, #1e293b);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 6px;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .form-row {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .form-label {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .form-input {
    background: var(--canvas-bg, #0f172a);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 4px;
    color: var(--text-primary, #e2e8f0);
    padding: 6px 8px;
    font-size: 0.857rem;
    outline: none;
    transition: border-color 0.15s;
  }

  .form-input:focus {
    border-color: var(--agent-accent, #06b6d4);
  }

  .form-input.mono {
    font-family: var(--mono-family, 'JetBrains Mono', monospace);
  }

  .form-textarea {
    resize: vertical;
    min-height: 48px;
    font-family: inherit;
  }

  select.form-input {
    cursor: pointer;
  }

  .btn-primary {
    background: var(--agent-accent, #06b6d4);
    border: none;
    color: var(--canvas-bg, #0f172a);
    padding: 8px 12px;
    border-radius: 6px;
    font-size: 0.786rem;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .btn-primary:hover:not(:disabled) {
    opacity: 0.85;
  }

  .btn-primary:disabled {
    opacity: 0.4;
    cursor: default;
  }

  /* Job list */
  .job-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .job-row {
    background: var(--card-bg, #1e293b);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 6px;
    overflow: hidden;
  }

  .job-row.disabled {
    opacity: 0.6;
  }

  .job-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px;
  }

  .job-status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
    background: var(--text-muted, #64748b);
  }

  .job-status-dot.active {
    background: var(--success-accent, #22c55e);
  }

  .job-info {
    flex: 1;
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
  }

  .job-name {
    font-size: 0.857rem;
    font-weight: 600;
    color: var(--text-primary, #e2e8f0);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .job-agent {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    flex-shrink: 0;
  }

  .job-controls {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  /* Toggle switch (matches PluginsTab) */
  .toggle-switch {
    width: 32px;
    height: 18px;
    background: var(--surface-border, #334155);
    border-radius: 9px;
    position: relative;
    cursor: pointer;
    border: none;
    padding: 0;
    transition: background 0.15s;
  }

  .toggle-switch.on {
    background: var(--agent-accent, #06b6d4);
  }

  .toggle-switch::after {
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

  .toggle-switch.on::after {
    transform: translateX(14px);
  }

  .toggle-switch:disabled {
    opacity: 0.5;
    cursor: default;
  }

  /* Job details */
  .job-details {
    padding: 0 10px 10px 10px;
    border-top: 1px solid var(--surface-border, #334155);
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-top: 8px;
  }

  .detail-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
  }

  .detail-cell {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .detail-label {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .detail-value {
    font-size: 0.786rem;
    color: var(--text-secondary, #94a3b8);
  }

  .detail-value.mono {
    font-family: var(--mono-family, 'JetBrains Mono', monospace);
    font-size: 0.714rem;
  }

  .job-message {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .msg-text {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 48px;
    overflow: hidden;
  }

  .job-actions {
    display: flex;
    gap: 6px;
    padding-top: 4px;
  }

  .btn-ghost {
    background: transparent;
    border: 1px solid var(--surface-border, #334155);
    color: var(--text-muted, #64748b);
    padding: 5px 12px;
    border-radius: 6px;
    font-size: 0.786rem;
    cursor: pointer;
    transition: border-color 0.15s, color 0.15s;
  }

  .btn-ghost:hover:not(:disabled) {
    border-color: var(--text-muted, #64748b);
    color: var(--text-secondary, #94a3b8);
  }

  .btn-ghost:disabled {
    opacity: 0.3;
    cursor: default;
  }

  .btn-sm {
    padding: 4px 8px;
    font-size: 0.714rem;
  }

  .btn-danger:hover:not(:disabled) {
    border-color: var(--alert-accent, #ef4444);
    color: var(--alert-accent, #ef4444);
  }

  /* Watches section */
  .section-divider {
    border-top: 1px solid var(--surface-border, #334155);
    margin: 16px 0 8px;
  }
  .section-title {
    font-size: 0.929rem;
    font-weight: 600;
    color: var(--text-primary, #e2e8f0);
    margin: 0 0 4px;
  }
  .watch-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .watch-item {
    border: 1px solid var(--surface-border, #334155);
    border-radius: 6px;
    padding: 8px 10px;
  }
  .watch-header {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .watch-info {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1;
    min-width: 0;
  }
  .watch-path {
    font-family: monospace;
    font-size: 0.786rem;
    color: var(--text-primary, #e2e8f0);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .watch-glob {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
  }
  .default-label {
    font-size: 0.643rem;
    color: var(--text-muted, #64748b);
    background: var(--surface-raised, #1e293b);
    padding: 1px 5px;
    border-radius: 3px;
  }
  .watch-details {
    display: flex;
    gap: 10px;
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    margin-top: 4px;
    padding-left: 44px;
  }
  .no-watches {
    font-size: 0.786rem;
    color: var(--text-muted, #64748b);
    text-align: center;
    padding: 12px;
  }
</style>
