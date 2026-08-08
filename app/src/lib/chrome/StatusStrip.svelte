<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { getAgents } from '$lib/stores/agents.svelte';
  import { getAllUsage, getDailyUsage } from '$lib/tauri/commands';
  import type { AgentStatus, AgentUsage, DailyUsage } from '$lib/tauri/commands';
  import { REFRESH_INTERVAL_MS } from '$lib/constants';

  // Count agents by broad status category
  let agents = $derived(getAgents());

  let workingCount = $derived(
    agents.filter(a => a.status === 'Working').length
  );
  let blockedCount = $derived(
    agents.filter(a => a.status === 'WaitingForHil').length
  );
  let idleCount = $derived(
    agents.filter(a => a.status === 'Idle').length
  );
  let totalAgents = $derived(agents.length);
  let errorCount = $derived(
    agents.filter(a => typeof a.status === 'object' && 'Error' in a.status).length
  );

  // Connection health: green if any agent is working/idle, yellow if all blocked, red if all error or no agents
  let connHealth = $derived.by((): 'ok' | 'degraded' | 'down' => {
    if (totalAgents === 0) return 'down';
    if (errorCount === totalAgents) return 'down';
    if (blockedCount > 0 && workingCount === 0 && idleCount === 0) return 'degraded';
    return 'ok';
  });

  // Usage data
  let allUsage: AgentUsage[] = $state([]);
  let dailyUsage: DailyUsage[] = $state([]);

  let totalCostCents = $derived(
    dailyUsage.reduce((sum, d) => sum + d.cost_usd_cents, 0)
  );
  let anyPricing = $derived(
    allUsage.some(a => a.has_pricing)
  );

  function formatCost(cents: number): string {
    return '$' + (cents / 100).toFixed(2);
  }

  async function refreshUsage() {
    try {
      const [usage, daily] = await Promise.all([getAllUsage(), getDailyUsage()]);
      allUsage = usage;
      dailyUsage = daily;
    } catch (_) { /* backend may not be ready yet */ }
  }

  // Session timer
  let sessionSeconds = $state(0);
  let timer: ReturnType<typeof setInterval>;
  let usageTimer: ReturnType<typeof setInterval>;

  onMount(() => {
    timer = setInterval(() => {
      sessionSeconds += 1;
    }, 1000);
    refreshUsage();
    usageTimer = setInterval(refreshUsage, REFRESH_INTERVAL_MS);
  });

  onDestroy(() => {
    clearInterval(timer);
    clearInterval(usageTimer);
  });

  function formatDuration(secs: number): string {
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = secs % 60;
    if (h > 0) {
      return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
    }
    return `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
  }
</script>

<div class="status-strip">
  <!-- Left: agent counts + telemetry -->
  <div class="left">
    {#if totalAgents === 0}
      <span class="label muted">No agents</span>
    {:else}
      {#if workingCount > 0}
        <span class="count working" title="Working">{workingCount}</span>
        <span class="label">working</span>
      {/if}
      {#if blockedCount > 0}
        <span class="count blocked" title="Awaiting approval">{blockedCount}</span>
        <span class="label">blocked</span>
      {/if}
      {#if idleCount > 0}
        <span class="count idle" title="Idle">{idleCount}</span>
        <span class="label">idle</span>
      {/if}
    {/if}

    {#if anyPricing}
      <span class="divider">|</span>
      <span class="label">cost:</span>
      <span class="value mono">{totalCostCents > 0 ? formatCost(totalCostCents) : '--'}</span>
    {/if}
  </div>

  <!-- Right: session timer + connection status -->
  <div class="right">
    <span class="label">session</span>
    <span class="value mono">{formatDuration(sessionSeconds)}</span>
    <span class="divider">|</span>
    <span
      class="conn-dot"
      class:conn-ok={connHealth === 'ok'}
      class:conn-degraded={connHealth === 'degraded'}
      class:conn-down={connHealth === 'down'}
      title={connHealth === 'ok' ? 'Agents online' : connHealth === 'degraded' ? 'All agents blocked' : 'No agents active'}
    ></span>
  </div>
</div>

<style>
  .status-strip {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    height: 100%;
    gap: 8px;
  }

  .left,
  .right {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .label {
    font-size: 0.786rem;
    color: var(--text-muted);
  }

  .muted {
    color: var(--text-muted);
    font-size: 0.786rem;
  }

  .value {
    font-size: 0.786rem;
    color: var(--text-secondary);
  }

  .mono {
    font-family: var(--mono-family);
  }

  .count {
    font-family: var(--mono-family);
    font-size: 0.857rem;
    font-weight: 700;
    min-width: 14px;
    text-align: center;
  }

  .count.working {
    color: var(--agent-accent);
  }

  .count.blocked {
    color: var(--alert-accent);
  }

  .count.idle {
    color: var(--text-muted);
  }

  .divider {
    color: var(--surface-border);
    font-size: 0.786rem;
    user-select: none;
    margin: 0 2px;
  }

  .conn-dot {
    display: inline-block;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-muted);
  }

  .conn-dot.conn-ok {
    background: var(--success-accent);
    box-shadow: 0 0 4px var(--success-accent);
  }

  .conn-dot.conn-degraded {
    background: var(--warning-accent, #f59e0b);
    box-shadow: 0 0 4px var(--warning-accent, #f59e0b);
  }

  .conn-dot.conn-down {
    background: var(--status-error, #ef4444);
    box-shadow: none;
  }
</style>
