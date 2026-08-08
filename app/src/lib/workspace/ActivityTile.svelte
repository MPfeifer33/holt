<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import {
    ACTIVITY_BADGE_LABELS,
    activityBadgeClass,
    activityResultColor,
    activityResultIcon,
    formatActivityTime,
  } from '$lib/workspace/activityFeed';
  import {
    getActivityEntries,
    ensureBackfill,
    registerScrollCallback,
  } from '$lib/stores/activity.svelte';

  interface Props {
    agentId: string;
  }

  let { agentId }: Props = $props();

  let entries = $derived(getActivityEntries(agentId));

  let feedEl: HTMLElement | undefined;
  let userScrolledUp = false;
  let unregisterScroll: (() => void) | null = null;

  function scrollToBottom() {
    if (!userScrolledUp && feedEl) {
      feedEl.scrollTop = feedEl.scrollHeight;
    }
  }

  onMount(async () => {
    // Backfill historical entries (idempotent — only runs once per agent)
    await ensureBackfill(agentId);

    // Register for auto-scroll notifications when new entries arrive
    unregisterScroll = registerScrollCallback(agentId, scrollToBottom);
  });

  onDestroy(() => {
    unregisterScroll?.();
  });

  function handleScroll() {
    if (!feedEl) return;
    const atBottom = feedEl.scrollTop + feedEl.clientHeight >= feedEl.scrollHeight - 16;
    userScrolledUp = !atBottom;
  }
</script>

<div class="activity-tile">
  <div class="tile-top">
    <span class="live-dot"></span>
    <span class="live-label">LIVE</span>
  </div>

  <div class="activity-feed" bind:this={feedEl} onscroll={handleScroll}>
    {#each entries as entry (entry.id)}
      <div class="activity-entry">
        <span class="entry-time">{formatActivityTime(entry.timestamp)}</span>
        <span class="entry-badge {activityBadgeClass(entry.type)}">{ACTIVITY_BADGE_LABELS[entry.type]}</span>
        <span class="entry-content">{entry.content}</span>
        {#if entry.result}
          <span class="entry-result" style="color: {activityResultColor(entry.result)}">{activityResultIcon(entry.result)}</span>
        {/if}
      </div>
    {/each}
    {#if entries.length === 0}
      <div class="empty-feed">No activity for this agent</div>
    {/if}
  </div>
</div>

<style>
  .activity-tile {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .tile-top {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    border-bottom: 1px solid var(--surface-border, #334155);
    flex-shrink: 0;
  }

  .live-dot {
    width: 6px;
    height: 6px;
    background: var(--success-accent, #22c55e);
    border-radius: 50%;
    animation: pulse 2s ease-in-out infinite;
  }

  .live-label {
    font-size: 0.714rem;
    font-family: var(--mono-family);
    color: var(--success-accent, #22c55e);
    font-weight: 600;
    letter-spacing: 0.5px;
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.4; }
  }

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
    font-family: var(--mono-family);
  }

  /* Badge colors */
  .badge-tool { background: rgba(6, 182, 212, 0.12); color: #06b6d4; }
  .badge-msg { background: rgba(99, 102, 241, 0.12); color: #6366f1; }
  .badge-hil { background: rgba(245, 158, 11, 0.12); color: #f59e0b; }
  .badge-veto { background: rgba(245, 158, 11, 0.12); color: #f59e0b; }
  .badge-err { background: rgba(239, 68, 68, 0.12); color: #ef4444; }
  .badge-status { background: rgba(34, 197, 94, 0.12); color: #22c55e; }
  .badge-sub { background: rgba(168, 85, 247, 0.12); color: #a855f7; }
  .badge-dream { background: rgba(244, 114, 182, 0.12); color: #f472b6; }
</style>
