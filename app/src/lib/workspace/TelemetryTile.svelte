<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { getAgentUsage, getConversation, getPricingConfig } from '$lib/tauri/commands';
  import type { AgentUsage, ModelPricing, ConversationMessageSummary } from '$lib/tauri/commands';
  import { REFRESH_INTERVAL_MS, CONTEXT_WARNING_PCT, CONTEXT_DANGER_PCT } from '$lib/constants';

  interface Props {
    agentId: string;
  }

  let { agentId }: Props = $props();

  // --- State ---

  let usage = $state<AgentUsage | null>(null);
  let pricing = $state<Record<string, ModelPricing>>({});
  let convStats = $state({ userTurns: 0, agentTurns: 0, toolCalls: 0 });
  let loading = $state(true);
  let error = $state<string | null>(null);
  let refreshInterval: ReturnType<typeof setInterval> | null = null;

  // --- Derived from real backend data ---

  let totalTokens = $derived((usage?.prompt_tokens ?? 0) + (usage?.completion_tokens ?? 0));
  let contextWindow = $derived(usage?.context_window_size ?? 0);
  let contextFillPct = $derived(
    contextWindow > 0 ? Math.min(100, Math.round((totalTokens / contextWindow) * 100)) : 0
  );
  let modelName = $derived(usage?.model_name ?? '');
  let hasTokenData = $derived(totalTokens > 0);

  // Cost: compute from real tokens + pricing config for this model
  let modelPricing = $derived(modelName ? findPricing(modelName) : null);
  let costUsd = $derived.by(() => {
    if (!modelPricing || !usage) return null;
    return (usage.prompt_tokens / 1_000_000) * modelPricing.input_per_million +
           (usage.completion_tokens / 1_000_000) * modelPricing.output_per_million;
  });

  let sessionDuration = $derived.by(() => {
    if (!usage?.session_start) return '—';
    const startMs = new Date(usage.session_start).getTime();
    const elapsedMs = Date.now() - startMs;
    const secs = Math.floor(elapsedMs / 1000);
    const mins = Math.floor(secs / 60);
    const hrs = Math.floor(mins / 60);
    if (hrs > 0) return `${hrs}h ${mins % 60}m`;
    if (mins > 0) return `${mins}m ${secs % 60}s`;
    return `${secs}s`;
  });

  // --- Helpers ---

  /** Match model name to pricing config. Tries exact match, then prefix match. */
  function findPricing(model: string): ModelPricing | null {
    if (pricing[model]) return pricing[model];
    // Try prefix match (e.g. "claude-opus-4-20260301" matches "claude-opus-4")
    for (const [key, value] of Object.entries(pricing)) {
      if (model.startsWith(key) || key.startsWith(model)) return value;
    }
    return null;
  }

  function formatTokens(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
    return String(n);
  }

  function formatCost(n: number): string {
    if (n < 0.01) return '<$0.01';
    return `$${n.toFixed(2)}`;
  }

  // --- Data loading ---

  async function refresh() {
    try {
      const [agentUsage, pricingConfig, conversation] = await Promise.all([
        getAgentUsage(agentId),
        getPricingConfig().catch(() => ({ global: {} })),
        getConversation(agentId),
      ]);

      usage = agentUsage;
      pricing = pricingConfig.global;

      // Count turns and tool calls from real conversation data
      let userTurns = 0;
      let agentTurns = 0;
      let toolCalls = 0;
      for (const msg of conversation) {
        if (msg.role === 'user') userTurns++;
        else if (msg.role === 'assistant') {
          agentTurns++;
          if (msg.tool_calls) toolCalls += msg.tool_calls.length;
        }
      }
      convStats = { userTurns, agentTurns, toolCalls };

      error = null;
    } catch (err) {
      error = String(err);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    refresh();
    refreshInterval = setInterval(refresh, REFRESH_INTERVAL_MS);
  });

  onDestroy(() => {
    if (refreshInterval !== null) {
      clearInterval(refreshInterval);
      refreshInterval = null;
    }
  });
</script>

<div class="telemetry-tile">
  {#if loading}
    <div class="loading">Loading…</div>
  {:else if error}
    <div class="error">{error}</div>
  {:else}
    <div class="metrics-grid">

      <!-- Token counts (API-reported) -->
      <div class="metric-group">
        <div class="group-label">
          Tokens
          {#if hasTokenData}
            <span class="source-badge">API</span>
          {/if}
        </div>
        {#if hasTokenData}
          <div class="metric-row">
            <span class="metric-key">Input</span>
            <span class="metric-val">{formatTokens(usage?.prompt_tokens ?? 0)}</span>
          </div>
          <div class="metric-row">
            <span class="metric-key">Output</span>
            <span class="metric-val">{formatTokens(usage?.completion_tokens ?? 0)}</span>
          </div>
          <div class="metric-row total-row">
            <span class="metric-key">Total</span>
            <span class="metric-val accent">{formatTokens(totalTokens)}</span>
          </div>
        {:else}
          <div class="no-data">No API usage reported yet</div>
        {/if}
      </div>

      <!-- Cost -->
      <div class="metric-group">
        <div class="group-label">Cost</div>
        {#if costUsd !== null}
          <div class="metric-cost">{formatCost(costUsd)}</div>
          <div class="cost-note">{modelName} rates</div>
        {:else if hasTokenData}
          <div class="no-data">No pricing configured for {modelName || 'this model'}</div>
        {:else}
          <div class="no-data">—</div>
        {/if}
      </div>

      <!-- Session stats -->
      <div class="metric-group">
        <div class="group-label">Session</div>
        <div class="metric-row">
          <span class="metric-key">Duration</span>
          <span class="metric-val">{sessionDuration}</span>
        </div>
        <div class="metric-row">
          <span class="metric-key">Messages</span>
          <span class="metric-val">{usage?.message_count ?? 0}</span>
        </div>
        <div class="metric-row">
          <span class="metric-key">User turns</span>
          <span class="metric-val">{convStats.userTurns}</span>
        </div>
        <div class="metric-row">
          <span class="metric-key">Agent turns</span>
          <span class="metric-val">{convStats.agentTurns}</span>
        </div>
        <div class="metric-row">
          <span class="metric-key">Tool calls</span>
          <span class="metric-val">{convStats.toolCalls}</span>
        </div>
        {#if modelName}
          <div class="metric-row">
            <span class="metric-key">Model</span>
            <span class="metric-val model-name">{modelName}</span>
          </div>
        {/if}
      </div>

      <!-- Context window fill -->
      <div class="metric-group context-group">
        <div class="group-label">Context Window</div>
        {#if contextWindow > 0}
          <div class="progress-bar-wrap">
            <div class="progress-bar">
              <div
                class="progress-fill"
                class:warn={contextFillPct > CONTEXT_WARNING_PCT}
                class:danger={contextFillPct > CONTEXT_DANGER_PCT}
                style="width: {contextFillPct}%"
              ></div>
            </div>
            <span class="progress-pct">{contextFillPct}%</span>
          </div>
          <div class="context-note">{formatTokens(totalTokens)} / {formatTokens(contextWindow)} tokens</div>
        {:else}
          <div class="no-data">Context window size not configured</div>
        {/if}
      </div>

    </div>
  {/if}
</div>

<style>
  .telemetry-tile {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow-y: auto;
    padding: 12px 14px;
  }

  .loading,
  .error {
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    color: var(--text-muted);
    margin: auto;
  }

  .error {
    color: var(--status-error, #ef4444);
  }

  .metrics-grid {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .metric-group {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .group-label {
    font-family: var(--mono-family, monospace);
    font-size: 0.714rem;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    font-weight: 600;
    padding-bottom: 4px;
    border-bottom: var(--border-width, 1px) solid var(--surface-border);
    margin-bottom: 2px;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .source-badge {
    font-size: 0.571rem;
    font-weight: 700;
    letter-spacing: 0.3px;
    padding: 1px 5px;
    border-radius: 3px;
    background: color-mix(in srgb, var(--success-accent, #22c55e) 15%, transparent);
    color: var(--success-accent, #22c55e);
  }

  .metric-row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    padding: 1px 0;
  }

  .total-row {
    margin-top: 2px;
    padding-top: 4px;
    border-top: var(--border-width, 1px) solid var(--surface-border);
  }

  .metric-key {
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    color: var(--text-secondary);
  }

  .metric-val {
    font-family: var(--mono-family, monospace);
    font-size: 0.929rem;
    color: var(--text-primary);
    font-weight: 600;
  }

  .metric-val.accent {
    color: var(--agent-accent, #06b6d4);
  }

  .metric-val.model-name {
    font-size: 0.786rem;
    font-weight: 500;
    color: var(--text-secondary);
    max-width: 160px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .metric-cost {
    font-family: var(--mono-family, monospace);
    font-size: 1.571rem;
    font-weight: 700;
    color: var(--text-primary);
    line-height: 1.2;
  }

  .cost-note {
    font-family: var(--mono-family, monospace);
    font-size: 0.714rem;
    color: var(--text-muted);
    opacity: 0.7;
  }

  .no-data {
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    color: var(--text-muted);
    opacity: 0.6;
    padding: 2px 0;
  }

  /* Context window progress bar */
  .context-group {
    gap: 6px;
  }

  .progress-bar-wrap {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .progress-bar {
    flex: 1;
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

  .progress-fill.danger {
    background: var(--status-error, #ef4444);
  }

  .progress-pct {
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    color: var(--text-secondary);
    min-width: 32px;
    text-align: right;
  }

  .context-note {
    font-family: var(--mono-family, monospace);
    font-size: 0.714rem;
    color: var(--text-muted);
    opacity: 0.7;
  }
</style>
