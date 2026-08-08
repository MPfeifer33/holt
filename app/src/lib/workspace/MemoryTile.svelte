<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { getMemoryStats, getMemoryEntries, searchMemories } from '$lib/tauri/commands';
  import type { MemoryStats as BackendStats, MemoryEntry } from '$lib/tauri/commands';
  import {
    REFRESH_INTERVAL_MS,
    MEMORY_QUERY_LIMIT,
    MEMORY_WARNING_PCT,
    DEFAULT_MEMORY_BUDGET_MAX,
    MEMORY_HOT_CAPACITY,
    DEBOUNCE_MS,
  } from '$lib/constants';

  interface Props {
    agentId: string;
  }

  let { agentId }: Props = $props();

  // --- Types ---

  interface MemoryStats {
    tiers: { hot: number; warm: number; cold: number; archive: number };
    budget: { used: number; max: number };
    totalStorage: number;
    totalCount: number;
    pinnedCount: number;
    provenance: { user: number; explicit: number; auto: number };
    status: 'online' | 'degraded' | 'offline';
  }

  interface InjectionEntry {
    id: string;
    timestamp: string;
    score: number;
    tier: 'hot' | 'warm' | 'cold';
    provenance: 'user_stated' | 'agent_explicit' | 'auto_captured';
    tokens: number;
    content: string;
    pinned: boolean;
  }

  // --- State ---

  let stats = $state<MemoryStats>({
    tiers: { hot: 0, warm: 0, cold: 0, archive: 0 },
    budget: { used: 0, max: DEFAULT_MEMORY_BUDGET_MAX },
    totalStorage: 0,
    totalCount: 0,
    pinnedCount: 0,
    provenance: { user: 0, explicit: 0, auto: 0 },
    status: 'offline',
  });
  let trace = $state<InjectionEntry[]>([]);

  // Search state
  let searchQuery = $state('');
  let searchResults = $state<InjectionEntry[]>([]);
  let searching = $state(false);
  let searchActive = $state(false);
  let searchDebounceTimer: ReturnType<typeof setTimeout> | null = null;

  // --- Data fetching ---

  function mapSource(source: string): InjectionEntry['provenance'] {
    if (source === 'user' || source === 'user_stated') return 'user_stated';
    if (source === 'agent' || source === 'agent_explicit') return 'agent_explicit';
    return 'auto_captured';
  }

  function mapTier(tier: string): InjectionEntry['tier'] {
    if (tier === 'hot') return 'hot';
    if (tier === 'warm') return 'warm';
    return 'cold';
  }

  function mapEntry(e: MemoryEntry): InjectionEntry {
    return {
      id: e.id,
      timestamp: e.created_at.split('T')[1]?.substring(0, 8) ?? e.created_at,
      score: e.score,
      tier: mapTier(e.tier),
      provenance: mapSource(e.source),
      tokens: e.tokens,
      content: e.content,
      pinned: e.pinned,
    };
  }

  async function refresh() {
    try {
      const [backendStats, entries] = await Promise.all([
        getMemoryStats(agentId),
        getMemoryEntries(agentId, MEMORY_QUERY_LIMIT),
      ]);

      // Map backend stats to display format
      const tc = backendStats.tier_counts;
      const provCounts = { user: 0, explicit: 0, auto: 0 };
      let totalTokens = 0;
      for (const e of entries) {
        totalTokens += e.tokens;
        const prov = mapSource(e.source);
        if (prov === 'user_stated') provCounts.user++;
        else if (prov === 'agent_explicit') provCounts.explicit++;
        else provCounts.auto++;
      }

      stats = {
        tiers: { hot: tc.hot, warm: tc.warm, cold: tc.cold, archive: tc.archive },
        budget: { used: backendStats.last_injection_tokens, max: backendStats.budget_max },
        totalStorage: backendStats.total_tokens,
        totalCount: tc.hot + tc.warm + tc.cold + tc.archive,
        pinnedCount: backendStats.pinned_count,
        provenance: provCounts,
        status: 'online',
      };

      trace = entries.map(mapEntry);
    } catch (_) {
      stats = { ...stats, status: 'offline' };
    }
  }

  async function doSearch(query: string) {
    if (!query.trim()) {
      searchResults = [];
      searchActive = false;
      searching = false;
      return;
    }
    searching = true;
    searchActive = true;
    try {
      const results = await searchMemories(agentId, query, 10);
      searchResults = results.map(mapEntry);
    } catch (_) {
      searchResults = [];
    } finally {
      searching = false;
    }
  }

  function onSearchInput() {
    if (searchDebounceTimer) clearTimeout(searchDebounceTimer);
    if (!searchQuery.trim()) {
      searchResults = [];
      searchActive = false;
      return;
    }
    searchDebounceTimer = setTimeout(() => doSearch(searchQuery), DEBOUNCE_MS);
  }

  function clearSearch() {
    searchQuery = '';
    searchResults = [];
    searchActive = false;
    if (searchDebounceTimer) clearTimeout(searchDebounceTimer);
  }

  let refreshInterval: ReturnType<typeof setInterval> | null = null;

  onMount(() => {
    refresh();
    refreshInterval = setInterval(refresh, REFRESH_INTERVAL_MS);
  });

  onDestroy(() => {
    if (refreshInterval !== null) {
      clearInterval(refreshInterval);
      refreshInterval = null;
    }
    if (searchDebounceTimer) clearTimeout(searchDebounceTimer);
  });

  // --- Derived values ---

  let budgetPct = $derived(Math.min(100, Math.round((stats.budget.used / stats.budget.max) * 100)));
  let provenanceTotal = $derived(stats.provenance.user + stats.provenance.explicit + stats.provenance.auto);
  let provPctUser = $derived(provenanceTotal > 0 ? (stats.provenance.user / provenanceTotal) * 100 : 0);
  let provPctExplicit = $derived(provenanceTotal > 0 ? (stats.provenance.explicit / provenanceTotal) * 100 : 0);
  let provPctAuto = $derived(provenanceTotal > 0 ? (stats.provenance.auto / provenanceTotal) * 100 : 0);

  const MAX_PINNED = 5;
  let pinnedSlots = $derived(
    Array.from({ length: MAX_PINNED }, (_, i) => i < stats.pinnedCount)
  );

  // --- Helpers ---

  const TIER_COLORS = {
    hot: 'var(--agent-accent, #06b6d4)',
    warm: 'var(--warning-accent, #f59e0b)',
    cold: '#6b7280',
    archive: '#374151',
  } as const;

  const PROVENANCE_LABELS: Record<InjectionEntry['provenance'], string> = {
    user_stated: 'USR',
    agent_explicit: 'AGT',
    auto_captured: 'AUTO',
  };

  const PROVENANCE_COLORS: Record<InjectionEntry['provenance'], string> = {
    user_stated: '#d946ef',
    agent_explicit: 'var(--agent-accent, #06b6d4)',
    auto_captured: '#6b7280',
  };

  const TIER_LABELS: Record<InjectionEntry['tier'], string> = {
    hot: 'HOT',
    warm: 'WRM',
    cold: 'CLD',
  };
</script>

<div class="memory-tile">
  <!-- Header -->
  <div class="header">
    <div class="header-left">
      <span class="title">MEMORY</span>
      <span class="total-count">{stats.totalCount.toLocaleString()}</span>
    </div>
    <span class="status-indicator" class:online={stats.status === 'online'} class:degraded={stats.status === 'degraded'} class:offline={stats.status === 'offline'}>
      <span class="status-dot"></span>
      {stats.status.toUpperCase()}
    </span>
  </div>

  <!-- Pinned Slots -->
  <div class="section">
    <div class="section-label">PINNED SLOTS</div>
    <div class="pinned-row">
      {#each pinnedSlots as filled, i}
        <div class="pin-slot" class:filled>
          <span class="pin-icon">{filled ? '\u{1F4CC}' : '\u{2013}'}</span>
          <span class="pin-label">{filled ? `Slot ${i + 1}` : 'Empty'}</span>
        </div>
      {/each}
    </div>
  </div>

  <!-- Tier Distribution -->
  <div class="section">
    <div class="section-label">TIER DISTRIBUTION</div>
    <div class="tier-row">
      {#each (['hot', 'warm', 'cold', 'archive'] as const) as tier}
        <div class="tier-box" style="border-color: {TIER_COLORS[tier]}">
          <span class="tier-label" style="color: {TIER_COLORS[tier]}">{tier.toUpperCase()}</span>
          <span class="tier-count">{stats.tiers[tier]}</span>
          {#if tier === 'hot'}
            <span class="tier-cap">(cap:{MEMORY_HOT_CAPACITY})</span>
          {/if}
        </div>
      {/each}
    </div>
  </div>

  <!-- Token Budget Bar -->
  <div class="section">
    <div class="section-label">LAST INJECTION</div>
    <div class="budget-row">
      <span class="budget-text">{stats.budget.used.toLocaleString()} / {stats.budget.max.toLocaleString()} tokens</span>
    </div>
    <div class="progress-bar">
      <div
        class="progress-fill"
        class:warn={budgetPct > MEMORY_WARNING_PCT}
        style="width: {budgetPct}%"
      ></div>
    </div>
    <div class="budget-meta">
      <span class="budget-note">GREEDY BY SCORE</span>
      <span class="storage-note">{stats.totalStorage.toLocaleString()} tokens in storage</span>
    </div>
  </div>

  <!-- Source Provenance Bar -->
  <div class="section">
    <div class="section-label">SOURCE PROVENANCE</div>
    <div class="provenance-bar">
      {#if provPctUser > 0}
        <div class="prov-segment prov-user" style="width: {provPctUser}%"></div>
      {/if}
      {#if provPctExplicit > 0}
        <div class="prov-segment prov-agent" style="width: {provPctExplicit}%"></div>
      {/if}
      {#if provPctAuto > 0}
        <div class="prov-segment prov-auto" style="width: {provPctAuto}%"></div>
      {/if}
    </div>
    <div class="prov-labels">
      <span class="prov-label" style="color: #d946ef">USR {stats.provenance.user}</span>
      <span class="prov-label" style="color: var(--agent-accent, #06b6d4)">AGT {stats.provenance.explicit}</span>
      <span class="prov-label" style="color: #6b7280">AUTO {stats.provenance.auto}</span>
    </div>
  </div>

  <!-- Search -->
  <div class="section">
    <div class="section-label">SEARCH MEMORIES</div>
    <div class="search-row">
      <input
        class="search-input"
        type="text"
        placeholder="Search by content..."
        bind:value={searchQuery}
        oninput={onSearchInput}
      />
      {#if searchQuery}
        <button class="search-clear" onclick={clearSearch}>&times;</button>
      {/if}
    </div>
    {#if searching}
      <div class="search-status">Searching...</div>
    {/if}
    {#if searchActive && !searching}
      <div class="search-status">{searchResults.length} result{searchResults.length !== 1 ? 's' : ''}</div>
    {/if}
  </div>

  <!-- Search Results or Injection Trace -->
  <div class="section trace-section">
    <div class="section-label">{searchActive ? 'SEARCH RESULTS' : 'LAST INJECTION TRACE'}</div>
    <div class="trace-list">
      {#each (searchActive ? searchResults : trace) as entry (entry.id)}
        <div class="trace-entry" class:pinned-entry={entry.pinned}>
          <div class="trace-meta">
            {#if entry.pinned}
              <span class="pin-indicator" title="Pinned">📌</span>
            {/if}
            <span class="trace-time">{entry.timestamp}</span>
            <span class="trace-badge tier-badge" style="background: {TIER_COLORS[entry.tier]}; color: #000">{TIER_LABELS[entry.tier]}</span>
            <span class="trace-badge prov-badge" style="background: {PROVENANCE_COLORS[entry.provenance]}; color: #000">{PROVENANCE_LABELS[entry.provenance]}</span>
            <span class="trace-score">{entry.score.toFixed(2)}</span>
            <span class="trace-tokens">{entry.tokens}t</span>
          </div>
          <div class="trace-content">{entry.content}</div>
        </div>
      {/each}
      {#if searchActive && searchResults.length === 0 && !searching}
        <div class="empty-state">No memories match this query.</div>
      {/if}
    </div>
  </div>
</div>

<style>
  .memory-tile {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow-y: auto;
    padding: 12px 14px;
    gap: 14px;
    font-family: var(--mono-family, monospace);
  }

  /* Header */
  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .header-left {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  .title {
    font-size: 0.929rem;
    font-weight: 700;
    color: var(--text-primary);
    letter-spacing: 1px;
  }

  .total-count {
    font-size: 0.786rem;
    color: var(--text-muted);
    font-weight: 600;
  }

  .status-indicator {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 0.714rem;
    font-weight: 600;
    letter-spacing: 0.5px;
    color: var(--text-muted);
  }

  .status-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-muted);
  }

  .status-indicator.online .status-dot {
    background: var(--agent-accent, #06b6d4);
    animation: pulse 2s ease-in-out infinite;
  }

  .status-indicator.online {
    color: var(--agent-accent, #06b6d4);
  }

  .status-indicator.degraded .status-dot {
    background: var(--warning-accent, #f59e0b);
  }

  .status-indicator.degraded {
    color: var(--warning-accent, #f59e0b);
  }

  .status-indicator.offline .status-dot {
    background: var(--status-error, #ef4444);
  }

  .status-indicator.offline {
    color: var(--status-error, #ef4444);
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.4; }
  }

  /* Sections */
  .section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .section-label {
    font-size: 0.714rem;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    font-weight: 600;
    padding-bottom: 4px;
    border-bottom: var(--border-width, 1px) solid var(--surface-border);
  }

  /* Pinned Slots */
  .pinned-row {
    display: flex;
    gap: 6px;
  }

  .pin-slot {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    padding: 6px 4px;
    border: 1px solid var(--surface-border);
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.02);
    opacity: 0.4;
    transition: opacity 200ms ease, border-color 200ms ease;
  }

  .pin-slot.filled {
    opacity: 1;
    border-color: var(--agent-accent, #06b6d4);
  }

  .pin-icon {
    font-size: 0.857rem;
    line-height: 1;
  }

  .pin-label {
    font-size: 0.571rem;
    color: var(--text-muted);
    letter-spacing: 0.3px;
    font-weight: 600;
  }

  /* Tier Distribution */
  .tier-row {
    display: flex;
    gap: 6px;
  }

  .tier-box {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    padding: 6px 4px;
    border: 1px solid;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.02);
  }

  .tier-label {
    font-size: 0.643rem;
    font-weight: 700;
    letter-spacing: 0.5px;
  }

  .tier-count {
    font-size: 1.286rem;
    font-weight: 700;
    color: var(--text-primary);
    line-height: 1.2;
  }

  .tier-cap {
    font-size: 0.571rem;
    color: var(--text-muted);
    opacity: 0.7;
  }

  /* Token Budget */
  .budget-row {
    display: flex;
    justify-content: flex-end;
  }

  .budget-text {
    font-size: 0.857rem;
    color: var(--text-secondary);
    font-weight: 600;
  }

  .progress-bar {
    height: 8px;
    background: var(--surface-border);
    border-radius: 4px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--agent-accent, #06b6d4);
    border-radius: 4px;
    transition: width 400ms ease;
  }

  .progress-fill.warn {
    background: var(--warning-accent, #f59e0b);
  }

  .budget-meta {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .budget-note {
    font-size: 0.643rem;
    color: var(--text-muted);
    opacity: 0.6;
    letter-spacing: 0.5px;
  }

  .storage-note {
    font-size: 0.643rem;
    color: var(--text-muted);
    opacity: 0.5;
  }

  /* Source Provenance */
  .provenance-bar {
    display: flex;
    height: 8px;
    border-radius: 4px;
    overflow: hidden;
    background: var(--surface-border);
  }

  .prov-segment {
    height: 100%;
    transition: width 400ms ease;
  }

  .prov-user {
    background: #d946ef;
  }

  .prov-agent {
    background: var(--agent-accent, #06b6d4);
  }

  .prov-auto {
    background: #6b7280;
  }

  .prov-labels {
    display: flex;
    gap: 12px;
  }

  .prov-label {
    font-size: 0.643rem;
    font-weight: 600;
    letter-spacing: 0.5px;
  }

  /* Search */
  .search-row {
    display: flex;
    align-items: center;
    gap: 4px;
    position: relative;
  }

  .search-input {
    flex: 1;
    background: rgba(255, 255, 255, 0.04);
    border: 1px solid var(--surface-border);
    border-radius: 4px;
    padding: 6px 28px 6px 8px;
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    color: var(--text-primary);
    outline: none;
    transition: border-color 200ms ease;
  }

  .search-input::placeholder {
    color: var(--text-muted);
    opacity: 0.5;
  }

  .search-input:focus {
    border-color: var(--agent-accent, #06b6d4);
  }

  .search-clear {
    position: absolute;
    right: 6px;
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: 1rem;
    cursor: pointer;
    padding: 0 4px;
    line-height: 1;
    opacity: 0.6;
    transition: opacity 150ms ease;
  }

  .search-clear:hover {
    opacity: 1;
    color: var(--text-primary);
  }

  .search-status {
    font-size: 0.643rem;
    color: var(--text-muted);
    letter-spacing: 0.3px;
  }

  /* Injection Trace */
  .trace-section {
    flex: 1;
    min-height: 0;
  }

  .trace-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 300px;
    overflow-y: auto;
    padding-right: 4px;
  }

  .trace-list::-webkit-scrollbar {
    width: 4px;
  }

  .trace-list::-webkit-scrollbar-track {
    background: transparent;
  }

  .trace-list::-webkit-scrollbar-thumb {
    background: var(--surface-border);
    border-radius: 2px;
  }

  .trace-entry {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 6px 8px;
    background: rgba(255, 255, 255, 0.02);
    border: 1px solid var(--surface-border);
    border-radius: 4px;
  }

  .trace-entry.pinned-entry {
    border-color: var(--agent-accent, #06b6d4);
    background: rgba(6, 182, 212, 0.04);
  }

  .trace-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .pin-indicator {
    font-size: 0.643rem;
    line-height: 1;
  }

  .trace-time {
    font-size: 0.714rem;
    color: var(--text-muted);
  }

  .trace-badge {
    font-size: 0.571rem;
    font-weight: 700;
    padding: 1px 5px;
    border-radius: 3px;
    letter-spacing: 0.3px;
  }

  .trace-score {
    font-size: 0.714rem;
    color: var(--agent-accent, #06b6d4);
    font-weight: 600;
  }

  .trace-tokens {
    font-size: 0.714rem;
    color: var(--text-muted);
    margin-left: auto;
  }

  .trace-content {
    font-size: 0.786rem;
    color: var(--text-secondary);
    line-height: 1.4;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }

  .empty-state {
    font-size: 0.714rem;
    color: var(--text-muted);
    text-align: center;
    padding: 12px;
    opacity: 0.6;
  }
</style>
