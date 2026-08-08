<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { getAgents } from '$lib/stores/agents.svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import {
    MAX_ACTIVITY_ENTRIES,
    TRUNC_MESSAGE,
  } from '$lib/constants';
  import {
    ACTIVITY_BADGE_LABELS,
    activityBadgeClass,
    activityResultColor,
    activityResultIcon,
    defaultAgentName,
    formatActivityTime,
    setupSharedActivityFeed,
  } from '$lib/workspace/activityFeed';
  import type { ActivityEntry } from '$lib/workspace/activityFeed';

  interface Props {
    focusedAgentId: string | null;
  }

  let { focusedAgentId }: Props = $props();

  const MAX_ENTRIES = MAX_ACTIVITY_ENTRIES;

  let entries = $state<ActivityEntry[]>([]);
  let expanded = $state(false);
  let filterAgent = $state<string | null>(null); // null = all
  let unlisteners: UnlistenFn[] = [];
  let feedEl = $state<HTMLElement | undefined>();
  let userScrolledUp = false;
  let cleanupFeed: (() => void) | null = null;

  // Derive latest entry for the collapsed bar
  let filteredEntries = $derived(
    filterAgent ? entries.filter(e => e.agentId === filterAgent) : entries,
  );
  let latestEntry = $derived(entries.length > 0 ? entries[entries.length - 1] : null);

  // Unique agent IDs present in entries for filter buttons
  let activeAgentIds = $derived([...new Set(entries.map(e => e.agentId))]);

  function agentName(agentId: string): string {
    return defaultAgentName(agentId, getAgents());
  }

  function addEntry(entry: Omit<ActivityEntry, 'id' | 'timestamp' | 'agentName'>) {
    const newEntry: ActivityEntry = {
      ...entry,
      id: `act-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
      timestamp: Date.now(),
      agentName: agentName(entry.agentId),
    };
    entries = [...entries.slice(-MAX_ENTRIES + 1), newEntry];

    // Auto-scroll if user hasn't scrolled up
    if (!userScrolledUp && feedEl) {
      tick().then(() => {
        if (feedEl) feedEl.scrollTop = feedEl.scrollHeight;
      });
    }
  }

  onMount(async () => {
    try {
      const { unlisteners: listeners, cleanup } = await setupSharedActivityFeed({
        acceptsAgent: () => true,
        addEntry,
        onError: (err) => console.error('ActivityDrawer: failed to set up event listeners:', err),
      });
      unlisteners = listeners;
      cleanupFeed = cleanup;
    } catch (err) {
      console.error('ActivityDrawer: failed to set up event listeners:', err);
    }
  });

  onDestroy(() => {
    for (const unlisten of unlisteners) {
      unlisten();
    }
    unlisteners = [];
    cleanupFeed?.();
  });

  function handleScroll() {
    if (!feedEl) return;
    const atBottom = feedEl.scrollTop + feedEl.clientHeight >= feedEl.scrollHeight - 16;
    userScrolledUp = !atBottom;
  }

  function toggleExpanded() {
    expanded = !expanded;
    if (expanded) {
      userScrolledUp = false;
      tick().then(() => {
        if (feedEl) feedEl.scrollTop = feedEl.scrollHeight;
      });
    }
  }
</script>

{#if !focusedAgentId}
  <div class="activity-drawer" class:expanded>
    <!-- Collapsed bar -->
    <button class="collapsed-bar" onclick={toggleExpanded}>
      <span class="chevron">{expanded ? '\u25BC' : '\u25B2'}</span>
      {#if latestEntry}
        <span class="latest-time">{formatActivityTime(latestEntry.timestamp)}</span>
        <span class="latest-agent">{latestEntry.agentName}</span>
        <span class="latest-badge {activityBadgeClass(latestEntry.type)}">{ACTIVITY_BADGE_LABELS[latestEntry.type]}</span>
        <span class="latest-content">{latestEntry.content}</span>
        {#if latestEntry.result}
          <span class="latest-result" style="color: {activityResultColor(latestEntry.result)}">{activityResultIcon(latestEntry.result)}</span>
        {/if}
      {:else}
        <span class="latest-content muted">No activity yet</span>
      {/if}
    </button>

    <!-- Expanded feed -->
    {#if expanded}
      <div class="expanded-panel">
        <!-- Filter bar -->
        <div class="filter-bar">
          <button
            class="filter-btn"
            class:active={filterAgent === null}
            onclick={() => { filterAgent = null; }}
          >All</button>
          {#each activeAgentIds as aid (aid)}
            <button
              class="filter-btn"
              class:active={filterAgent === aid}
              onclick={() => { filterAgent = aid; }}
            >{agentName(aid)}</button>
          {/each}
        </div>

        <!-- Feed -->
        <div class="activity-feed" bind:this={feedEl} onscroll={handleScroll}>
          {#each filteredEntries as entry (entry.id)}
            <div class="activity-entry">
              <span class="entry-time">{formatActivityTime(entry.timestamp)}</span>
              <span class="entry-agent">{entry.agentName}</span>
              <span class="entry-badge {activityBadgeClass(entry.type)}">{ACTIVITY_BADGE_LABELS[entry.type]}</span>
              <span class="entry-content">{entry.content}</span>
              {#if entry.result}
                <span class="entry-result" style="color: {activityResultColor(entry.result)}">{activityResultIcon(entry.result)}</span>
              {/if}
            </div>
          {/each}
          {#if filteredEntries.length === 0}
            <div class="empty-feed">No activity{filterAgent ? ' for this agent' : ''}</div>
          {/if}
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .activity-drawer {
    position: absolute;
    bottom: 28px; /* sits above the status strip */
    left: 0;
    right: 0;
    z-index: 39;
    display: flex;
    flex-direction: column;
    pointer-events: auto;
  }

  /* Collapsed bar */
  .collapsed-bar {
    height: 28px;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px;
    background: var(--card-bg, #1e293b);
    border-top: var(--border-width, 1px) solid var(--surface-border, #334155);
    border-bottom: none;
    border-left: none;
    border-right: none;
    cursor: pointer;
    font-family: var(--mono-family);
    font-size: 0.786rem;
    color: var(--text-secondary, #94a3b8);
    text-align: left;
    transition: background 0.15s;
    width: 100%;
    overflow: hidden;
  }

  .collapsed-bar:hover {
    background: var(--surface-border, #334155);
  }

  .chevron {
    font-size: 0.571rem;
    color: var(--text-muted, #64748b);
    flex-shrink: 0;
  }

  .latest-time {
    color: var(--text-muted, #475569);
    font-size: 0.714rem;
    flex-shrink: 0;
  }

  .latest-agent {
    font-weight: 600;
    font-size: 0.714rem;
    color: var(--agent-accent, #06b6d4);
    flex-shrink: 0;
  }

  .latest-badge {
    font-size: 0.643rem;
    padding: 1px 5px;
    border-radius: 3px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    flex-shrink: 0;
  }

  .latest-content {
    font-size: 0.714rem;
    color: var(--text-secondary, #94a3b8);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
  }

  .latest-content.muted {
    color: var(--text-muted, #64748b);
  }

  .latest-result {
    font-size: 0.714rem;
    flex-shrink: 0;
  }

  /* Expanded panel */
  .expanded-panel {
    height: 200px;
    display: flex;
    flex-direction: column;
    background: var(--card-bg, #1e293b);
    border-top: var(--border-width, 1px) solid var(--surface-border, #334155);
    animation: drawer-expand 0.15s ease-out;
  }

  @keyframes drawer-expand {
    from {
      height: 0;
      opacity: 0;
    }
    to {
      height: 200px;
      opacity: 1;
    }
  }

  /* Filter bar */
  .filter-bar {
    display: flex;
    gap: 4px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--surface-border, #334155);
    flex-shrink: 0;
    overflow-x: auto;
  }

  .filter-btn {
    font-size: 0.714rem;
    font-family: var(--mono-family);
    padding: 2px 8px;
    border-radius: 4px;
    border: 1px solid var(--surface-border, #334155);
    background: transparent;
    color: var(--text-muted, #64748b);
    cursor: pointer;
    white-space: nowrap;
    transition: background 0.1s, color 0.1s, border-color 0.1s;
  }

  .filter-btn:hover {
    color: var(--text-secondary, #94a3b8);
  }

  .filter-btn.active {
    background: rgba(6, 182, 212, 0.12);
    border-color: var(--agent-accent, #06b6d4);
    color: var(--agent-accent, #06b6d4);
  }

  /* Activity feed */
  .activity-feed {
    flex: 1;
    overflow-y: auto;
    padding: 4px 8px;
    font-family: var(--mono-family);
  }

  .activity-entry {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 4px;
    border-bottom: 1px solid var(--canvas-bg, #0f172a);
    animation: entry-slide 0.2s ease-out;
  }

  .activity-entry:last-child {
    border-bottom: none;
  }

  .activity-entry:hover {
    background: var(--canvas-bg, #0f172a);
  }

  @keyframes entry-slide {
    from { opacity: 0; transform: translateY(-4px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .entry-time {
    color: var(--text-muted, #475569);
    font-size: 0.714rem;
    white-space: nowrap;
    min-width: 52px;
    flex-shrink: 0;
  }

  .entry-agent {
    font-size: 0.714rem;
    font-weight: 600;
    white-space: nowrap;
    min-width: 80px;
    flex-shrink: 0;
    color: var(--agent-accent, #06b6d4);
  }

  .entry-badge {
    font-size: 0.643rem;
    padding: 1px 5px;
    border-radius: 3px;
    font-weight: 600;
    white-space: nowrap;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    flex-shrink: 0;
  }

  .entry-content {
    color: var(--text-secondary, #94a3b8);
    font-size: 0.714rem;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .entry-result {
    font-size: 0.714rem;
    white-space: nowrap;
    flex-shrink: 0;
  }

  .empty-feed {
    font-size: 0.786rem;
    color: var(--text-muted, #64748b);
    text-align: center;
    padding: 24px 0;
  }

  /* Badge colors */
  .badge-tool { background: rgba(6, 182, 212, 0.12); color: #06b6d4; }
  .badge-msg { background: rgba(99, 102, 241, 0.12); color: #6366f1; }
  .badge-hil { background: rgba(245, 158, 11, 0.12); color: #f59e0b; }
  .badge-veto { background: rgba(245, 158, 11, 0.12); color: #f59e0b; }
  .badge-err { background: rgba(239, 68, 68, 0.12); color: #ef4444; }
  .badge-status { background: rgba(34, 197, 94, 0.12); color: #22c55e; }
  .badge-sub { background: rgba(168, 85, 247, 0.12); color: #a855f7; }
</style>
