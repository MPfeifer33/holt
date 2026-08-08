<script lang="ts">
  import { onMount } from 'svelte';
  import {
    getAllUsage,
    getAgentUsage,
    getDailyUsage,
    getContextStatus,
    compactAgentContext,
    queryRecentTraces,
    exportTraces,
    clearTraces,
    getPricingConfig,
    listAgents,
    parseAgentStatus,
    type AgentUsage,
    type AgentSummary,
    type DailyUsage,
    type ContextStatus,
    type TraceEntry,
    type PricingConfig,
    type ModelPricing,
  } from '$lib/tauri/commands';
  import { REFRESH_INTERVAL_MS, OBSERVABILITY_TRACE_LIMIT } from '$lib/constants';

  // --- Section collapse state ---
  let sections = $state({
    totals: true,
    perAgent: true,
    daily: true,
    context: true,
    traces: true,
    pricing: false,
  });

  function toggleSection(key: keyof typeof sections) {
    sections[key] = !sections[key];
  }

  // =====================
  // State
  // =====================

  let agents = $state<AgentSummary[]>([]);
  let allUsage = $state<AgentUsage[]>([]);
  let dailyUsage = $state<DailyUsage[]>([]);
  let traceEntries = $state<TraceEntry[]>([]);
  let pricingConfig = $state<PricingConfig | null>(null);
  let contextStatus = $state<ContextStatus | null>(null);
  let selectedContextAgentId = $state<string | null>(null);
  let compactingContext = $state(false);
  let compactResult = $state<string | null>(null);

  // Trace filter
  type TraceFilter = 'all' | 'errors' | 'tools';
  let traceFilter = $state<TraceFilter>('all');
  let confirmClear = $state(false);

  // Session timer
  let sessionStartTime = $state(Date.now());
  let sessionElapsed = $state(0);

  // Loading / error
  let loadingUsage = $state(true);
  let loadError = $state<string | null>(null);

  // Refresh interval handle
  let refreshInterval: ReturnType<typeof setInterval> | undefined;
  let timerInterval: ReturnType<typeof setInterval> | undefined;

  // =====================
  // Derived
  // =====================

  let totalTokens = $derived(
    allUsage.reduce((sum, a) => sum + a.prompt_tokens + a.completion_tokens, 0)
  );

  let totalMessages = $derived(
    allUsage.reduce((sum, a) => sum + a.message_count, 0)
  );

  // We don't have cost from getAllUsage directly — approximate from daily if available
  let totalCostCents = $derived(
    dailyUsage.reduce((sum, d) => sum + d.cost_usd_cents, 0)
  );

  let anyPricing = $derived(
    allUsage.some(a => a.has_pricing)
  );

  let filteredTraces = $derived(
    traceEntries.filter((t) => {
      if (traceFilter === 'all') return true;
      if (traceFilter === 'errors') return t.trace_type === 'error' || t.outcome === 'failure';
      if (traceFilter === 'tools') return t.trace_type === 'tool_call' || t.trace_type === 'tool';
      return true;
    })
  );

  let contextFillPercent = $derived(
    contextStatus && contextStatus.context_tokens > 0
      ? Math.round(
          (contextStatus.context_tokens / contextStatus.context_window_size) * 100
        )
      : -1
  );

  let contextMaxTokens = $derived(
    contextStatus ? contextStatus.context_window_size : 0
  );

  // =====================
  // Data fetching
  // =====================

  async function refreshData() {
    try {
      const [agentList, usage, daily, traces] = await Promise.all([
        listAgents(),
        getAllUsage(),
        getDailyUsage(),
        queryRecentTraces(OBSERVABILITY_TRACE_LIMIT),
      ]);
      agents = agentList;
      allUsage = usage;
      dailyUsage = daily;
      traceEntries = traces;
      loadError = null;

      // Auto-select first agent for context if none selected
      if (!selectedContextAgentId && agentList.length > 0) {
        selectedContextAgentId = agentList[0].id;
      }

      // Refresh context status if agent selected
      if (selectedContextAgentId) {
        await refreshContextStatus();
      }
    } catch (e) {
      loadError = e instanceof Error ? e.message : String(e);
    } finally {
      loadingUsage = false;
    }
  }

  async function refreshContextStatus() {
    if (!selectedContextAgentId) return;
    try {
      contextStatus = await getContextStatus(selectedContextAgentId);
    } catch {
      contextStatus = null;
    }
  }

  async function handleCompactContext() {
    if (!selectedContextAgentId || compactingContext) return;
    compactingContext = true;
    compactResult = null;
    try {
      compactResult = await compactAgentContext(selectedContextAgentId);
      await refreshContextStatus();
    } catch (e) {
      compactResult = e instanceof Error ? e.message : String(e);
    } finally {
      compactingContext = false;
    }
  }

  async function handleExportTraces() {
    try {
      const path = await exportTraces();
      compactResult = `Exported to ${path}`;
      setTimeout(() => { compactResult = null; }, 3000);
    } catch (e) {
      loadError = e instanceof Error ? e.message : String(e);
    }
  }

  async function handleClearTraces() {
    if (!confirmClear) {
      confirmClear = true;
      return;
    }
    try {
      await clearTraces();
      traceEntries = [];
      confirmClear = false;
    } catch (e) {
      loadError = e instanceof Error ? e.message : String(e);
      confirmClear = false;
    }
  }

  async function loadPricing() {
    try {
      pricingConfig = await getPricingConfig();
    } catch {
      pricingConfig = null;
    }
  }

  // =====================
  // Formatting helpers
  // =====================

  function formatTokens(n: number): string {
    if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M';
    if (n >= 1_000) return (n / 1_000).toFixed(1) + 'K';
    return n.toString();
  }

  function formatCost(cents: number): string {
    return '$' + (cents / 100).toFixed(2);
  }

  function formatDuration(ms: number): string {
    const totalSec = Math.floor(ms / 1000);
    const h = Math.floor(totalSec / 3600);
    const m = Math.floor((totalSec % 3600) / 60);
    const s = totalSec % 60;
    if (h > 0) return `${h}h ${m}m`;
    if (m > 0) return `${m}m ${s}s`;
    return `${s}s`;
  }

  function formatTimestamp(ts: string): string {
    try {
      const d = new Date(ts);
      return d.toLocaleTimeString('en-US', { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' });
    } catch {
      return ts;
    }
  }

  function formatDate(dateStr: string): string {
    try {
      const d = new Date(dateStr);
      return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
    } catch {
      return dateStr;
    }
  }

  function traceTypeBadge(traceType: string): { label: string; color: string } {
    const t = traceType.toLowerCase();
    if (t.includes('tool')) return { label: 'TOOL', color: 'var(--agent-accent, #06b6d4)' };
    if (t.includes('api') || t.includes('completion')) return { label: 'API', color: 'var(--infra-accent, #a855f7)' };
    if (t.includes('error') || t.includes('err')) return { label: 'ERR', color: 'var(--alert-color, #ef4444)' };
    return { label: 'SYS', color: 'var(--text-muted, #64748b)' };
  }

  function agentColorCss(usage: AgentUsage): string {
    const c = usage.agent_color;
    return `rgb(${c.r}, ${c.g}, ${c.b})`;
  }

  function agentNameForTrace(agentId: string): string {
    const agent = agents.find(a => a.id === agentId);
    return agent ? agent.name : agentId.slice(0, 8);
  }

  // =====================
  // Lifecycle
  // =====================

  function startPolling() {
    if (!refreshInterval) {
      refreshInterval = setInterval(refreshData, REFRESH_INTERVAL_MS);
    }
    if (!timerInterval) {
      timerInterval = setInterval(() => {
        sessionElapsed = Date.now() - sessionStartTime;
      }, 1000);
    }
  }

  function stopPolling() {
    if (refreshInterval) {
      clearInterval(refreshInterval);
      refreshInterval = undefined;
    }
    if (timerInterval) {
      clearInterval(timerInterval);
      timerInterval = undefined;
    }
  }

  function handleVisibilityChange() {
    if (document.hidden) {
      stopPolling();
    } else {
      sessionElapsed = Date.now() - sessionStartTime;
      refreshData();
      startPolling();
    }
  }

  onMount(() => {
    sessionStartTime = Date.now();
    refreshData();
    loadPricing();
    startPolling();

    document.addEventListener('visibilitychange', handleVisibilityChange);

    return () => {
      stopPolling();
      document.removeEventListener('visibilitychange', handleVisibilityChange);
    };
  });
</script>

<div class="observe-tab">
  <!-- ===== 1. Session Totals ===== -->
  <button class="section-header" onclick={() => toggleSection('totals')}>
    <span class="section-chevron" class:open={sections.totals}></span>
    <span class="section-dot" style="background: var(--agent-accent, #06b6d4);"></span>
    Session Totals
  </button>
  {#if sections.totals}
    <div class="totals-grid">
      <div class="total-card">
        <div class="total-label">Total Tokens</div>
        <div class="total-value">{loadingUsage ? '--' : formatTokens(totalTokens)}</div>
      </div>
      <div class="total-card">
        <div class="total-label">Total Cost</div>
        <div class="total-value">{loadingUsage ? '--' : formatCost(totalCostCents)}</div>
      </div>
      <div class="total-card">
        <div class="total-label">Messages</div>
        <div class="total-value">{loadingUsage ? '--' : totalMessages.toLocaleString()}</div>
      </div>
      <div class="total-card">
        <div class="total-label">Session</div>
        <div class="total-value">{formatDuration(sessionElapsed)}</div>
      </div>
    </div>
  {/if}

  <!-- ===== 2. Per-Agent Usage ===== -->
  <button class="section-header" onclick={() => toggleSection('perAgent')}>
    <span class="section-chevron" class:open={sections.perAgent}></span>
    <span class="section-dot" style="background: var(--infra-accent, #6366f1);"></span>
    Per-Agent Usage
  </button>
  {#if sections.perAgent}
    <div class="section-body">
      {#if allUsage.length === 0}
        <p class="empty-msg">{loadingUsage ? 'Loading...' : 'No agents with usage data'}</p>
      {:else}
        <div class="usage-table">
          <div class="usage-header">
            <span class="col-name">Agent</span>
            <span class="col-num">Prompt</span>
            <span class="col-num">Compl.</span>
            <span class="col-num">Total</span>
          </div>
          {#each allUsage as usage (usage.agent_id)}
            <div class="usage-row">
              <span class="col-name">
                <span class="agent-dot" style="background: {agentColorCss(usage)};"></span>
                <span class="agent-name-text">{usage.agent_name}</span>
              </span>
              <span class="col-num mono">{formatTokens(usage.prompt_tokens)}</span>
              <span class="col-num mono">{formatTokens(usage.completion_tokens)}</span>
              <span class="col-num mono">{formatTokens(usage.prompt_tokens + usage.completion_tokens)}</span>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}

  <!-- ===== 3. Daily Usage History ===== -->
  <button class="section-header" onclick={() => toggleSection('daily')}>
    <span class="section-chevron" class:open={sections.daily}></span>
    <span class="section-dot" style="background: var(--success-color, #22c55e);"></span>
    Daily Usage
  </button>
  {#if sections.daily}
    <div class="section-body">
      {#if dailyUsage.length === 0}
        <p class="empty-msg">{loadingUsage ? 'Loading...' : 'No daily usage data'}</p>
      {:else}
        <div class="usage-table">
          <div class="usage-header">
            <span class="col-name">Date</span>
            <span class="col-num">Tokens</span>
            {#if anyPricing}
              <span class="col-num">Cost</span>
            {/if}
          </div>
          {#each dailyUsage as day (day.date)}
            <div class="usage-row">
              <span class="col-name">{formatDate(day.date)}</span>
              <span class="col-num mono">{formatTokens(day.prompt_tokens + day.completion_tokens)}</span>
              {#if anyPricing}
                <span class="col-num mono">{formatCost(day.cost_usd_cents)}</span>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}

  <!-- ===== 4. Context Window ===== -->
  <button class="section-header" onclick={() => toggleSection('context')}>
    <span class="section-chevron" class:open={sections.context}></span>
    <span class="section-dot" style="background: var(--warning-color, #f59e0b);"></span>
    Context Window
  </button>
  {#if sections.context}
    <div class="section-body">
      <!-- Agent selector -->
      <div class="context-selector">
        <select
          class="agent-select"
          value={selectedContextAgentId ?? ''}
          onchange={(e) => {
            const target = e.target as HTMLSelectElement;
            selectedContextAgentId = target.value || null;
            refreshContextStatus();
          }}
        >
          <option value="" disabled>Select agent</option>
          {#each agents as agent (agent.id)}
            <option value={agent.id}>{agent.name}</option>
          {/each}
        </select>
      </div>

      {#if contextStatus && contextFillPercent >= 0}
        <div class="context-info">
          <div class="context-bar-header">
            <span class="mono">{formatTokens(contextStatus.context_tokens)} / {formatTokens(contextMaxTokens)}</span>
            <span class="mono">{contextFillPercent}%</span>
          </div>
          <div class="context-bar-track">
            <div
              class="context-bar-fill"
              class:warning={contextFillPercent > 75}
              class:danger={contextFillPercent > 90}
              style="width: {contextFillPercent}%;"
            ></div>
          </div>
          <div class="context-details">
            <span>Messages: <span class="mono">{contextStatus.message_count}</span></span>
            {#if contextStatus.needs_compaction}
              <span class="needs-compact">Needs compaction</span>
            {/if}
          </div>
        </div>
      {:else if contextStatus}
        <div class="context-info">
          <div class="context-details muted">
            <span>Messages: <span class="mono">{contextStatus.message_count}</span></span>
          </div>
        </div>
        <button
          class="btn-action"
          onclick={handleCompactContext}
          disabled={compactingContext}
        >
          {compactingContext ? 'Compacting...' : 'Compact Context'}
        </button>
        {#if compactResult}
          <p class="inline-result">{compactResult}</p>
        {/if}
      {:else if selectedContextAgentId}
        <p class="empty-msg">Loading context status...</p>
      {:else}
        <p class="empty-msg">Select an agent above</p>
      {/if}
    </div>
  {/if}

  <!-- ===== 5. Trace Log ===== -->
  <button class="section-header" onclick={() => toggleSection('traces')}>
    <span class="section-chevron" class:open={sections.traces}></span>
    <span class="section-dot" style="background: var(--infra-accent, #a855f7);"></span>
    Trace Log
  </button>
  {#if sections.traces}
    <div class="section-body">
      <!-- Filter tabs -->
      <div class="filter-tabs">
        <button class="filter-btn" class:active={traceFilter === 'all'} onclick={() => { traceFilter = 'all'; }}>All</button>
        <button class="filter-btn" class:active={traceFilter === 'errors'} onclick={() => { traceFilter = 'errors'; }}>Errors</button>
        <button class="filter-btn" class:active={traceFilter === 'tools'} onclick={() => { traceFilter = 'tools'; }}>Tool Calls</button>
      </div>

      <!-- Trace entries -->
      <div class="trace-log">
        {#if filteredTraces.length === 0}
          <p class="empty-msg">{loadingUsage ? 'Loading...' : 'No matching traces'}</p>
        {:else}
          {#each filteredTraces as entry (entry.id)}
            {@const badge = traceTypeBadge(entry.trace_type)}
            <div class="trace-row">
              <span class="trace-time">{formatTimestamp(entry.timestamp)}</span>
              <span class="trace-badge" style="color: {badge.color};">{badge.label}</span>
              <span class="trace-agent">{agentNameForTrace(entry.agent_id)}</span>
              <span class="trace-outcome" class:success={entry.outcome === 'success'} class:failure={entry.outcome === 'failure'}>
                {entry.outcome === 'success' ? '\u2713' : entry.outcome === 'failure' ? '\u2717' : '\u2014'}
              </span>
              <span class="trace-content" title={entry.content}>{entry.content}</span>
            </div>
          {/each}
        {/if}
      </div>

      <!-- Actions -->
      <div class="trace-actions">
        <button class="btn-action" onclick={handleExportTraces}>Export</button>
        <button
          class="btn-action btn-danger"
          onclick={handleClearTraces}
        >
          {confirmClear ? 'Confirm Clear?' : 'Clear'}
        </button>
      </div>
    </div>
  {/if}

  <!-- ===== 6. Pricing Config ===== -->
  <button class="section-header" onclick={() => { toggleSection('pricing'); if (!pricingConfig) loadPricing(); }}>
    <span class="section-chevron" class:open={sections.pricing}></span>
    <span class="section-dot" style="background: var(--text-muted, #64748b);"></span>
    Pricing Config
  </button>
  {#if sections.pricing}
    <div class="section-body">
      {#if pricingConfig && Object.keys(pricingConfig.global).length > 0}
        <div class="usage-table">
          <div class="usage-header">
            <span class="col-name">Model</span>
            <span class="col-num">In $/1M</span>
            <span class="col-num">Out $/1M</span>
          </div>
          {#each Object.entries(pricingConfig.global) as [model, pricing] (model)}
            <div class="usage-row">
              <span class="col-name mono" title={model}>{model}</span>
              <span class="col-num mono">${(pricing as ModelPricing).input_per_million.toFixed(2)}</span>
              <span class="col-num mono">${(pricing as ModelPricing).output_per_million.toFixed(2)}</span>
            </div>
          {/each}
        </div>
      {:else}
        <p class="empty-msg">No pricing data available</p>
      {/if}
    </div>
  {/if}

  <!-- Error display -->
  {#if loadError}
    <div class="error-bar">
      <span>{loadError}</span>
      <button class="dismiss-btn" onclick={() => { loadError = null; }}>Dismiss</button>
    </div>
  {/if}
</div>

<style>
  .observe-tab {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  /* --- Section headers --- */
  .section-header {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 4px;
    border: none;
    background: transparent;
    color: var(--text-primary, #e2e8f0);
    font-size: 0.786rem;
    font-weight: 600;
    cursor: pointer;
    letter-spacing: 0.3px;
    text-transform: uppercase;
    transition: background 0.15s;
    border-radius: 4px;
  }

  .section-header:hover {
    background: var(--card-bg, #1e293b);
  }

  .section-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .section-chevron {
    display: inline-block;
    width: 0;
    height: 0;
    border-left: 4px solid var(--text-muted, #64748b);
    border-top: 3px solid transparent;
    border-bottom: 3px solid transparent;
    transition: transform 0.15s;
    flex-shrink: 0;
  }

  .section-chevron.open {
    transform: rotate(90deg);
  }

  .section-body {
    padding: 4px 0 12px 0;
  }

  /* --- Session totals grid --- */
  .totals-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    padding: 4px 0 12px 0;
  }

  .total-card {
    background: var(--card-bg, #1e293b);
    border-radius: 8px;
    padding: 10px;
    text-align: center;
  }

  .total-label {
    font-size: 0.643rem;
    color: var(--text-muted, #64748b);
    text-transform: uppercase;
    letter-spacing: 1px;
    margin-bottom: 4px;
  }

  .total-value {
    font-size: 1.429rem;
    font-weight: 700;
    color: var(--text-primary, #e2e8f0);
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
  }

  /* --- Usage tables --- */
  .usage-table {
    font-size: 0.786rem;
    width: 100%;
  }

  .usage-header {
    display: flex;
    padding: 6px 0;
    border-bottom: 1px solid var(--card-bg, #1e293b);
    color: var(--text-muted, #64748b);
    font-weight: 500;
  }

  .usage-row {
    display: flex;
    padding: 6px 0;
    border-bottom: 1px solid var(--card-bg, #1e293b);
    align-items: center;
    transition: background 0.1s;
  }

  .usage-row:hover {
    background: rgba(255, 255, 255, 0.02);
  }

  .col-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .col-num {
    width: 56px;
    text-align: right;
    flex-shrink: 0;
  }

  .mono {
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
  }

  /* --- Agent dots --- */
  .agent-dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    margin-right: 4px;
    vertical-align: middle;
    flex-shrink: 0;
  }

  .agent-name-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* --- Context window --- */
  .context-selector {
    margin-bottom: 8px;
  }

  .agent-select {
    width: 100%;
    padding: 6px 8px;
    background: var(--card-bg, #1e293b);
    color: var(--text-primary, #e2e8f0);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 6px;
    font-size: 0.786rem;
    font-family: inherit;
    outline: none;
    cursor: pointer;
  }

  .agent-select:focus {
    border-color: var(--agent-accent, #06b6d4);
  }

  .context-info {
    background: var(--card-bg, #1e293b);
    border-radius: 6px;
    padding: 8px;
    margin-bottom: 8px;
  }

  .context-bar-header {
    display: flex;
    justify-content: space-between;
    font-size: 0.714rem;
    color: var(--text-secondary, #94a3b8);
    margin-bottom: 4px;
  }

  .context-bar-track {
    height: 8px;
    background: var(--surface-border, #334155);
    border-radius: 4px;
    overflow: hidden;
  }

  .context-bar-fill {
    height: 100%;
    background: linear-gradient(90deg, var(--agent-accent, #06b6d4), var(--success-color, #22c55e));
    border-radius: 4px;
    transition: width 0.3s ease;
  }

  .context-bar-fill.warning {
    background: linear-gradient(90deg, var(--warning-color, #f59e0b), var(--alert-color, #ef4444));
  }

  .context-bar-fill.danger {
    background: var(--alert-color, #ef4444);
  }

  .context-details {
    display: flex;
    gap: 12px;
    margin-top: 6px;
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
  }

  .needs-compact {
    color: var(--warning-color, #f59e0b);
    font-weight: 600;
  }

  /* --- Trace log --- */
  .filter-tabs {
    display: flex;
    gap: 4px;
    margin-bottom: 8px;
  }

  .filter-btn {
    padding: 4px 10px;
    border: 1px solid var(--surface-border, #334155);
    border-radius: 4px;
    background: transparent;
    color: var(--text-muted, #64748b);
    font-size: 0.714rem;
    cursor: pointer;
    transition: all 0.15s;
  }

  .filter-btn:hover {
    background: var(--card-bg, #1e293b);
    color: var(--text-secondary, #94a3b8);
  }

  .filter-btn.active {
    background: rgba(6, 182, 212, 0.1);
    border-color: var(--agent-accent, #06b6d4);
    color: var(--agent-accent, #06b6d4);
  }

  .trace-log {
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    max-height: 200px;
    overflow-y: auto;
    background: var(--canvas-bg, #0f172a);
    border-radius: 6px;
    padding: 6px;
    border: 1px solid var(--card-bg, #1e293b);
  }

  .trace-row {
    display: flex;
    gap: 6px;
    padding: 3px 0;
    border-bottom: 1px solid var(--card-bg, #1e293b);
    align-items: baseline;
  }

  .trace-row:last-child {
    border-bottom: none;
  }

  .trace-time {
    color: var(--surface-border, #475569);
    flex-shrink: 0;
    width: 56px;
  }

  .trace-badge {
    font-weight: 700;
    flex-shrink: 0;
    width: 32px;
  }

  .trace-agent {
    color: var(--text-secondary, #94a3b8);
    flex-shrink: 0;
    max-width: 60px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .trace-outcome {
    flex-shrink: 0;
    width: 12px;
    text-align: center;
  }

  .trace-outcome.success {
    color: var(--success-color, #22c55e);
  }

  .trace-outcome.failure {
    color: var(--alert-color, #ef4444);
  }

  .trace-content {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted, #64748b);
  }

  .trace-actions {
    display: flex;
    gap: 6px;
    margin-top: 8px;
  }

  /* --- Buttons --- */
  .btn-action {
    padding: 5px 12px;
    font-size: 0.714rem;
    border: 1px solid var(--surface-border, #334155);
    border-radius: 6px;
    background: transparent;
    color: var(--text-secondary, #94a3b8);
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-action:hover:not(:disabled) {
    background: var(--card-bg, #1e293b);
    color: var(--text-primary, #e2e8f0);
  }

  .btn-action:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-danger {
    border-color: var(--alert-color, #ef4444);
    color: var(--alert-color, #ef4444);
  }

  .btn-danger:hover:not(:disabled) {
    background: rgba(239, 68, 68, 0.1);
    color: var(--alert-color, #ef4444);
  }

  /* --- Misc --- */
  .empty-msg {
    color: var(--text-muted, #64748b);
    font-size: 0.786rem;
    text-align: center;
    padding: 16px 0;
  }

  .inline-result {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    margin-top: 4px;
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
  }

  .dismiss-btn {
    padding: 2px 8px;
    border: 1px solid var(--surface-border, #334155);
    border-radius: 4px;
    background: transparent;
    color: var(--text-secondary, #94a3b8);
    cursor: pointer;
    font-size: 0.714rem;
  }

  .error-bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 10px;
    margin-top: 8px;
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid var(--alert-color, #ef4444);
    border-radius: 6px;
    color: var(--alert-color, #ef4444);
    font-size: 0.786rem;
  }
</style>
