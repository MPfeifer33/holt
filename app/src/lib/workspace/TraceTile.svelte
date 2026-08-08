<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { queryRecentTraces } from '$lib/tauri/commands';
  import type { TraceEntry } from '$lib/tauri/commands';
  import { TRACE_REFRESH_INTERVAL_MS, TRACE_QUERY_LIMIT, TRUNC_MESSAGE } from '$lib/constants';

  interface Props {
    agentId: string;
  }

  let { agentId }: Props = $props();

  type TabFilter = 'all' | 'errors' | 'tool_calls';

  const TABS: { id: TabFilter; label: string }[] = [
    { id: 'all', label: 'All' },
    { id: 'errors', label: 'Errors' },
    { id: 'tool_calls', label: 'Tool Calls' },
  ];

  let allEntries = $state<TraceEntry[]>([]);
  let activeTab = $state<TabFilter>('all');
  let loading = $state(true);
  let error = $state<string | null>(null);
  let refreshInterval: ReturnType<typeof setInterval> | null = null;

  let filteredEntries = $derived.by(() => {
    switch (activeTab) {
      case 'errors':
        return allEntries.filter(e =>
          e.trace_type === 'error' ||
          e.outcome === 'failure'
        );
      case 'tool_calls':
        return allEntries.filter(e =>
          e.trace_type === 'tool_call' ||
          e.trace_type === 'tool_result'
        );
      default:
        return allEntries;
    }
  });

  async function refresh() {
    try {
      allEntries = await queryRecentTraces(TRACE_QUERY_LIMIT, agentId);
      error = null;
    } catch (err) {
      error = String(err);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    refresh();
    refreshInterval = setInterval(refresh, TRACE_REFRESH_INTERVAL_MS);
  });

  onDestroy(() => {
    if (refreshInterval !== null) {
      clearInterval(refreshInterval);
      refreshInterval = null;
    }
  });

  function formatTimestamp(ts: string): string {
    try {
      const d = new Date(ts);
      return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
    } catch {
      return ts;
    }
  }

  function truncate(text: string, maxLen = TRUNC_MESSAGE): string {
    if (text.length <= maxLen) return text;
    return text.slice(0, maxLen) + '\u2026';
  }

  function badgeClass(traceType: string): string {
    const t = traceType.toLowerCase();
    if (t.includes('error') || t.includes('fail')) return 'badge-error';
    if (t.includes('tool')) return 'badge-tool';
    if (t.includes('token') || t.includes('llm')) return 'badge-llm';
    return 'badge-default';
  }
</script>

<div class="trace-tile">
  <!-- Tab bar -->
  <div class="tab-bar">
    {#each TABS as tab}
      <button
        class="tab-btn"
        class:active={activeTab === tab.id}
        onclick={() => { activeTab = tab.id; }}
      >
        {tab.label}
      </button>
    {/each}
  </div>

  <!-- Trace list -->
  <div class="trace-list">
    {#if loading}
      <div class="state-msg">Loading traces…</div>
    {:else if error}
      <div class="state-msg error">{error}</div>
    {:else if filteredEntries.length === 0}
      <div class="state-msg">No entries for this filter.</div>
    {:else}
      {#each filteredEntries as entry (entry.id)}
        <div class="trace-entry">
          <div class="entry-meta">
            <span class="entry-time">{formatTimestamp(entry.timestamp)}</span>
            <span class="entry-badge {badgeClass(entry.trace_type)}">{entry.trace_type}</span>
            {#if entry.outcome}
              <span class="entry-outcome" class:outcome-ok={entry.outcome === 'success'} class:outcome-err={entry.outcome === 'failure'}>
                {entry.outcome}
              </span>
            {/if}
          </div>
          <div class="entry-content">{truncate(entry.content)}</div>
        </div>
      {/each}
    {/if}
  </div>
</div>

<style>
  .trace-tile {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }

  /* Tab bar */
  .tab-bar {
    display: flex;
    gap: 0;
    border-bottom: var(--border-width, 1px) solid var(--surface-border);
    flex-shrink: 0;
    padding: 0 10px;
  }

  .tab-btn {
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    padding: 6px 12px;
    border: none;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
    transition: color 120ms, border-color 120ms;
  }

  .tab-btn:hover {
    color: var(--text-secondary);
  }

  .tab-btn.active {
    color: var(--agent-accent, #06b6d4);
    border-bottom-color: var(--agent-accent, #06b6d4);
  }

  /* Trace list */
  .trace-list {
    flex: 1;
    overflow-y: auto;
    padding: 6px 0;
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

  /* Trace entry */
  .trace-entry {
    padding: 6px 12px;
    border-bottom: var(--border-width, 1px) solid var(--surface-border);
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .trace-entry:last-child {
    border-bottom: none;
  }

  .trace-entry:hover {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 5%, transparent);
  }

  .entry-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .entry-time {
    font-family: var(--mono-family, monospace);
    font-size: 0.714rem;
    color: var(--text-muted);
    opacity: 0.7;
    flex-shrink: 0;
  }

  .entry-badge {
    font-family: var(--mono-family, monospace);
    font-size: 0.643rem;
    text-transform: uppercase;
    letter-spacing: 0.3px;
    padding: 1px 6px;
    border-radius: 3px;
    font-weight: 700;
  }

  .badge-error {
    background: color-mix(in srgb, var(--status-error, #ef4444) 20%, transparent);
    color: var(--status-error, #ef4444);
  }

  .badge-tool {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 20%, transparent);
    color: var(--agent-accent, #06b6d4);
  }

  .badge-llm {
    background: color-mix(in srgb, var(--success-accent, #22c55e) 20%, transparent);
    color: var(--success-accent, #22c55e);
  }

  .badge-default {
    background: var(--surface-border);
    color: var(--text-muted);
  }

  .entry-outcome {
    font-family: var(--mono-family, monospace);
    font-size: 0.643rem;
    padding: 1px 5px;
    border-radius: 3px;
    opacity: 0.85;
  }

  .outcome-ok {
    background: color-mix(in srgb, var(--success-accent, #22c55e) 15%, transparent);
    color: var(--success-accent, #22c55e);
  }

  .outcome-err {
    background: color-mix(in srgb, var(--status-error, #ef4444) 15%, transparent);
    color: var(--status-error, #ef4444);
  }

  .entry-content {
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    color: var(--text-secondary);
    line-height: 1.4;
    white-space: pre-wrap;
    word-break: break-all;
  }
</style>
