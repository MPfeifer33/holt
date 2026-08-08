<script lang="ts">
  import type { CanvasAgent, SubagentInfo } from '$lib/stores/agents.svelte';
  import { isGraphView, getZoom } from '$lib/stores/canvas.svelte';
  import { getHighestAttentionForAgent } from '$lib/stores/attention.svelte';
  import { DRAG_THRESHOLD } from '$lib/constants';

  let {
    agent,
    subagents = [],
    onshowcontextmenu,
    onstartconnection,
    onmove,
    onfocus,
  }: {
    agent: CanvasAgent;
    subagents?: SubagentInfo[];
    onshowcontextmenu?: (x: number, y: number, agentId: string) => void;
    onstartconnection?: (agentId: string, startX: number, startY: number) => void;
    onmove?: (agentId: string, x: number, y: number) => void;
    onfocus?: (agentId: string) => void;
  } = $props();

  let graphView = $derived(isGraphView());

  let attention = $derived(getHighestAttentionForAgent(agent.id));
  let isCriticalAttention = $derived(attention?.priority === 'critical');

  let isWorking = $derived(agent.status === 'Working');
  let isBlocked = $derived(
    agent.status === 'WaitingForHil' ||
    (typeof agent.status === 'object' && 'Error' in agent.status)
  );

  let agentRgb = $derived(
    agent.color ? `rgb(${agent.color.r}, ${agent.color.g}, ${agent.color.b})` : 'var(--agent-accent)'
  );

  // Thinking status text — derived from latest activity
  let thinkingText = $derived.by(() => {
    if (!isWorking) return '';
    const recent = agent.recentActivity;
    if (recent && recent.length > 0) {
      const last = recent[recent.length - 1].line;
      // Clean up common prefixes for a tighter display
      return last.replace(/^> /, '');
    }
    return 'thinking...';
  });

  let statusColor = $derived(
    isBlocked ? 'var(--alert-accent)' :
    isWorking ? agentRgb :
    agentRgb
  );

  let statusLabel = $derived(
    agent.status === 'Working' ? 'Working' :
    agent.status === 'WaitingForHil' ? 'Needs approval' :
    agent.status === 'Idle' ? 'Idle' :
    typeof agent.status === 'object' && 'WaitingForAgent' in agent.status ? 'Waiting' :
    typeof agent.status === 'object' && 'Error' in agent.status ? 'Error' :
    'Unknown'
  );

  // Drag-to-move state
  // DRAG_THRESHOLD imported from constants
  let isDragging = $state(false);
  let dragStart: { x: number; y: number; agentX: number; agentY: number } | null = null;
  let totalMovement = 0;

  function handlePointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();

    dragStart = { x: e.clientX, y: e.clientY, agentX: agent.x, agentY: agent.y };
    totalMovement = 0;
    isDragging = false;

    const target = e.currentTarget as HTMLElement;
    target.setPointerCapture(e.pointerId);
    target.addEventListener('pointermove', handlePointerMove);
    target.addEventListener('pointerup', handlePointerUp);
  }

  function handlePointerMove(e: PointerEvent) {
    if (!dragStart) return;

    const dx = e.clientX - dragStart.x;
    const dy = e.clientY - dragStart.y;
    totalMovement += Math.abs(dx) + Math.abs(dy);

    if (totalMovement > DRAG_THRESHOLD) {
      isDragging = true;
    }

    if (isDragging) {
      const zoom = getZoom();
      const newX = dragStart.agentX + (e.clientX - dragStart.x) / zoom;
      const newY = dragStart.agentY + (e.clientY - dragStart.y) / zoom;
      onmove?.(agent.id, newX, newY);
    }
  }

  function handlePointerUp(e: PointerEvent) {
    const target = e.currentTarget as HTMLElement;
    target.releasePointerCapture(e.pointerId);
    target.removeEventListener('pointermove', handlePointerMove);
    target.removeEventListener('pointerup', handlePointerUp);

    const wasDragging = isDragging;
    dragStart = null;
    isDragging = false;
    totalMovement = 0;

    // If it was not a drag, treat as click
    if (!wasDragging) {
      handleClick();
    }
  }

  function handleClick() {
    onfocus?.(agent.id);
  }

  function handleDblClick() {
    if (isDragging) return;
    onfocus?.(agent.id);
  }

  function handleContextMenu(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    onshowcontextmenu?.(e.clientX, e.clientY, agent.id);
  }

  // Subagent badge (desk view)
  let hasSubagents = $derived(subagents.length > 0);
  let subagentListOpen = $state(false);

  function subagentStatusColor(status: string): string {
    if (status === 'Working' || status === 'running') return 'var(--agent-accent)';
    if (status === 'WaitingForHil' || status === 'blocked') return 'var(--alert-accent)';
    return 'var(--surface-border)';
  }

  // Connection handle drag
  let showHandle = $state(false);

  function handleHandlePointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    // Coords passed here are unused — parent derives source position from
    // agent position + dimensions (see pendingLineSource in +page.svelte).
    onstartconnection?.(agent.id, agent.x, agent.y);
  }
</script>

{#if graphView}
  <!-- Graph View: circle node -->
  <div
    class="agent-circle"
    class:working={isWorking}
    class:blocked={isBlocked}
    class:dragging={isDragging}
    style="left: {agent.x}px; top: {agent.y}px; --status-color: {statusColor};"
    role="button"
    tabindex="0"
    data-node-type="agent"
    data-node-id={agent.id}
    onpointerdown={handlePointerDown}
    onkeydown={(e) => { if (e.key === 'Enter') handleClick(); }}
    oncontextmenu={handleContextMenu}
  >
    <span class="circle-icon">{isWorking ? '\u25CE' : isBlocked ? '\u26A0' : '\u25CB'}</span>
    {#if isWorking}
      <div class="orbit-ring">
        <span class="orbit-dot"></span>
        <span class="orbit-dot" style="animation-delay: -1s"></span>
        <span class="orbit-dot" style="animation-delay: -2s"></span>
      </div>
    {/if}
    {#if isBlocked}
      <span class="ping-dot"></span>
    {/if}
    {#if isCriticalAttention}
      <div class="radar-ping" style="border-color: rgb({agent.color.r}, {agent.color.g}, {agent.color.b})"></div>
    {/if}
  </div>
{:else}
  <!-- Desk View: card node -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="agent-card"
    class:working={isWorking}
    class:blocked={isBlocked}
    class:dragging={isDragging}
    style="left: {agent.x}px; top: {agent.y}px; --status-color: {statusColor};"
    role="button"
    tabindex="0"
    data-node-type="agent"
    data-node-id={agent.id}
    onpointerdown={handlePointerDown}
    ondblclick={handleDblClick}
    onkeydown={(e) => { if (e.key === 'Enter') handleClick(); }}
    oncontextmenu={handleContextMenu}
    onmouseenter={() => showHandle = true}
    onmouseleave={() => showHandle = false}
  >
    {#if isBlocked}
      <div class="blocked-badge">{'\u26A0'}</div>
    {/if}

    {#if hasSubagents}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="subagent-badge"
        title="{subagents.length} subagent{subagents.length === 1 ? '' : 's'}"
        onpointerdown={(e) => { e.stopPropagation(); subagentListOpen = !subagentListOpen; }}
      >{subagents.length} sub</div>
    {/if}

    <div class="card-header">
      <div class="card-icon" style="background: color-mix(in srgb, {statusColor} 20%, transparent);">
        <span style="color: {statusColor};">{'\u25C8'}</span>
      </div>
      <div class="card-title-group">
        <h3 class="card-name">
          {agent.name}
          {#if isWorking}
            <span class="thinking-dots-inline">
              <span class="tdot"></span>
              <span class="tdot"></span>
              <span class="tdot"></span>
            </span>
          {/if}
        </h3>
        <p class="card-status">{statusLabel}</p>
      </div>
    </div>

    <div class="card-log">
      <div class="log-dots">
        <span class="dot"></span>
        <span class="dot"></span>
      </div>
      <div class="log-lines">
        {#if agent.recentActivity && agent.recentActivity.length > 0}
          {#each agent.recentActivity.slice(-3) as act}
            <div class="log-line">&gt; {act.line}</div>
          {/each}
        {:else}
          <div class="log-line">&gt; ready</div>
        {/if}
        {#if isWorking}
          <div class="thinking-bar">
            <div class="dot-grid">
              {#each {length: 9} as _, i}
                <span class="grid-dot" style="animation-delay: {i * 0.08}s"></span>
              {/each}
            </div>
            <span class="thinking-text">{thinkingText}</span>
          </div>
        {:else if isBlocked}
          <div class="log-line last blocked-line">&gt; awaiting approval</div>
        {/if}
      </div>
    </div>

    {#if hasSubagents && subagentListOpen}
      <div class="subagent-list">
        {#each subagents as sa (sa.id)}
          <div class="subagent-row">
            <span class="subagent-dot" style="background: {subagentStatusColor(sa.status)};"></span>
            <span class="subagent-name">{sa.name}</span>
            <span class="subagent-status">{sa.status}</span>
          </div>
        {/each}
      </div>
    {/if}

    <!-- Connection handle (right edge) -->
    {#if showHandle || isDragging}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="connection-handle"
        class:visible={showHandle}
        onpointerdown={handleHandlePointerDown}
        title="Drag to connect"
      ></div>
    {/if}
  </div>
{/if}

<style>
  /* ── Graph View: Circle ── */
  .agent-circle {
    position: absolute;
    width: 64px;
    height: 64px;
    border-radius: 50%;
    border: 4px solid var(--status-color);
    background: var(--card-bg);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition: transform 0.2s ease, border-color 0.2s ease, box-shadow 0.2s ease;
    transform: translate(-50%, -50%);
    user-select: none;
    overflow: visible;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.25);
  }

  .agent-circle:hover {
    transform: translate(-50%, -50%) scale(1.12);
  }

  .agent-circle.dragging {
    cursor: grabbing;
    transition: none;
    transform: translate(-50%, -50%) scale(1.05);
  }

  .agent-circle.working {
    box-shadow: 0 0 20px color-mix(in srgb, var(--agent-accent) 40%, transparent);
  }

  .agent-circle.blocked {
    box-shadow: 0 0 16px color-mix(in srgb, var(--alert-accent) 30%, transparent);
  }

  .circle-icon {
    font-size: 1.714rem;
    color: var(--status-color);
    line-height: 1;
  }

  .ping-dot {
    position: absolute;
    top: -2px;
    right: -2px;
    width: 14px;
    height: 14px;
    background: var(--alert-accent);
    border-radius: 50%;
    animation: ping 1.5s cubic-bezier(0, 0, 0.2, 1) infinite;
  }

  @keyframes ping {
    0% { transform: scale(1); opacity: 1; }
    75%, 100% { transform: scale(1.8); opacity: 0; }
  }

  .radar-ping {
    position: absolute;
    top: 50%;
    left: 50%;
    width: 100%;
    height: 100%;
    border-radius: 50%;
    border: 2px solid;
    transform: translate(-50%, -50%);
    animation: radar-ping 2s cubic-bezier(0, 0, 0.2, 1) infinite;
    pointer-events: none;
  }

  @keyframes radar-ping {
    0% { transform: translate(-50%, -50%) scale(1); opacity: 0.4; }
    100% { transform: translate(-50%, -50%) scale(2.5); opacity: 0; }
  }

  /* ── Desk View: Card ── */
  .agent-card {
    position: absolute;
    width: 280px;
    min-height: 120px;
    overflow: visible;
    background: var(--card-bg);
    border: var(--border-width) solid var(--surface-border);
    border-left: 3px solid var(--status-color);
    border-radius: var(--border-radius);
    padding: 14px;
    cursor: pointer;
    transition: transform 0.2s ease, border-color 0.2s ease, box-shadow 0.2s ease;
    transform: translate(-50%, -50%);
    user-select: none;
    box-shadow: 0 2px 12px rgba(0, 0, 0, 0.2);
  }

  .agent-card:hover {
    transform: translate(-50%, -50%) translateY(-3px);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
  }

  .agent-card.dragging {
    cursor: grabbing;
    transition: none;
    transform: translate(-50%, -50%);
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.4);
    z-index: 10;
  }

  .agent-card.working {
    border-left-color: var(--agent-accent);
    box-shadow: 0 0 20px color-mix(in srgb, var(--agent-accent) 12%, transparent);
  }

  .agent-card.working:hover {
    border-color: var(--agent-accent);
  }

  .agent-card.blocked {
    border-left-color: var(--alert-accent);
    box-shadow: 0 0 16px color-mix(in srgb, var(--alert-accent) 12%, transparent);
  }

  .agent-card.blocked:hover {
    border-color: var(--alert-accent);
  }

  .blocked-badge {
    position: absolute;
    top: -10px;
    right: -10px;
    width: 28px;
    height: 28px;
    background: var(--alert-accent);
    color: #fff;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 1rem;
    animation: bounce 1s ease infinite;
    box-shadow: 0 2px 8px color-mix(in srgb, var(--alert-accent) 40%, transparent);
  }

  @keyframes bounce {
    0%, 100% { transform: translateY(0); }
    50% { transform: translateY(-4px); }
  }

  .card-header {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    margin-bottom: 10px;
  }

  .card-icon {
    width: 32px;
    height: 32px;
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 1.143rem;
    flex-shrink: 0;
  }

  .card-title-group {
    min-width: 0;
  }

  .card-name {
    font-size: 0.929rem;
    font-weight: 600;
    color: var(--text-primary);
    margin: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    max-width: 200px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* ── Thinking Dots (inline, next to name) ── */
  .thinking-dots-inline {
    display: inline-flex;
    gap: 3px;
    align-items: center;
    margin-left: 2px;
  }

  .tdot {
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: var(--agent-accent);
    animation: dot-pulse 1.4s ease-in-out infinite;
  }

  .tdot:nth-child(2) { animation-delay: 0.2s; }
  .tdot:nth-child(3) { animation-delay: 0.4s; }

  @keyframes dot-pulse {
    0%, 80%, 100% {
      opacity: 0.25;
      transform: scale(0.8);
    }
    40% {
      opacity: 1;
      transform: scale(1.3);
      background: #fff;
    }
  }

  .card-status {
    font-size: 0.786rem;
    color: var(--text-secondary);
    margin: 2px 0 0 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .card-log {
    background: color-mix(in srgb, var(--canvas-bg) 90%, transparent);
    border: var(--border-width) solid color-mix(in srgb, var(--surface-border) 50%, transparent);
    border-radius: 6px;
    padding: 8px;
    font-family: var(--mono-family);
    font-size: 0.714rem;
    color: var(--text-muted);
    position: relative;
    min-height: 48px;
    overflow: hidden;
  }

  .log-dots {
    position: absolute;
    top: 6px;
    right: 8px;
    display: flex;
    gap: 3px;
  }

  .dot {
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: var(--surface-border);
  }

  .log-lines {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .log-line {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .log-line.last {
    color: var(--text-secondary);
  }

  .blocked-line {
    color: var(--alert-accent) !important;
  }

  /* ── Thinking Bar (desk view, replaces "processing...") ── */
  .thinking-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 2px 0;
    min-width: 0; /* allow flex shrink below child min-content */
  }

  .dot-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 2px;
    flex-shrink: 0;
  }

  .grid-dot {
    width: 3px;
    height: 3px;
    border-radius: 50%;
    background: var(--agent-accent);
    animation: grid-cascade 1.2s ease-in-out infinite;
  }

  @keyframes grid-cascade {
    0%, 70%, 100% {
      opacity: 0.15;
      transform: scale(0.7);
    }
    35% {
      opacity: 1;
      transform: scale(1.4);
      background: #e0f2fe;
    }
  }

  .thinking-text {
    font-family: var(--mono-family);
    font-size: 0.714rem;
    color: var(--agent-accent);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    animation: text-fade 2s ease-in-out infinite;
    min-width: 0; /* critical: lets text-overflow: ellipsis actually engage */
    flex: 1 1 auto; /* take remaining space in the thinking-bar row */
  }

  @keyframes text-fade {
    0%, 100% { opacity: 0.6; }
    50% { opacity: 1; }
  }

  /* ── Orbital Dots (graph view, when working) ── */
  .orbit-ring {
    position: absolute;
    top: 50%;
    left: 50%;
    width: 80px;
    height: 80px;
    transform: translate(-50%, -50%);
    pointer-events: none;
    animation: orbit-spin 3s linear infinite;
  }

  .orbit-dot {
    position: absolute;
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--agent-accent);
    box-shadow: 0 0 6px var(--agent-accent);
  }

  .orbit-dot:nth-child(1) {
    top: 0;
    left: 50%;
    transform: translateX(-50%);
    background: #fff;
  }

  .orbit-dot:nth-child(2) {
    bottom: 12%;
    right: 5%;
    background: var(--agent-accent);
    opacity: 0.7;
  }

  .orbit-dot:nth-child(3) {
    bottom: 12%;
    left: 5%;
    opacity: 0.4;
  }

  @keyframes orbit-spin {
    from { transform: translate(-50%, -50%) rotate(0deg); }
    to { transform: translate(-50%, -50%) rotate(360deg); }
  }

  /* ── Subagent Badge (desk view) ── */
  .subagent-badge {
    position: absolute;
    top: -8px;
    right: 22px; /* offset left of blocked-badge to avoid overlap */
    background: color-mix(in srgb, var(--agent-accent) 20%, var(--card-bg));
    border: 1px solid var(--agent-accent);
    color: var(--agent-accent);
    border-radius: 10px;
    padding: 1px 7px;
    font-size: 0.714rem;
    font-weight: 600;
    font-family: var(--mono-family);
    cursor: pointer;
    user-select: none;
    white-space: nowrap;
  }

  .subagent-badge:hover {
    background: color-mix(in srgb, var(--agent-accent) 30%, var(--card-bg));
  }

  /* ── Subagent Collapsible List (desk view) ── */
  .subagent-list {
    margin-top: 6px;
    border: var(--border-width) solid color-mix(in srgb, var(--surface-border) 50%, transparent);
    border-radius: 6px;
    padding: 4px 6px;
    background: color-mix(in srgb, var(--canvas-bg) 80%, transparent);
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .subagent-row {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 0.714rem;
    font-family: var(--mono-family);
  }

  .subagent-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .subagent-name {
    color: var(--text-secondary);
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .subagent-status {
    color: var(--text-muted);
    font-size: 0.643rem;
    flex-shrink: 0;
  }

  /* ── Connection Handle ── */
  .connection-handle {
    position: absolute;
    right: -6px;
    top: 50%;
    transform: translateY(-50%);
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--surface-border);
    border: 2px solid var(--card-bg);
    cursor: crosshair;
    opacity: 0;
    transition: opacity 0.15s ease, background 0.15s ease;
    z-index: 5;
  }

  .connection-handle.visible {
    opacity: 1;
  }

  .connection-handle:hover {
    background: var(--agent-accent);
    transform: translateY(-50%) scale(1.3);
  }
</style>
