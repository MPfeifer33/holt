<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { setFocusedAgent } from '$lib/stores/canvas.svelte';
  import { parseAgentStatus, statusText, type AgentStatus } from '$lib/tauri/commands';
  import ChatTile from './ChatTile.svelte';
  import TelemetryTile from './TelemetryTile.svelte';
  import TraceTile from './TraceTile.svelte';
  import FileBrowserTile from './FileBrowserTile.svelte';
  import ActivityTile from './ActivityTile.svelte';
  import MemoryTile from './MemoryTile.svelte';
  import CheckpointTile from './CheckpointTile.svelte';
  import ClaudeTuiTile from './ClaudeTuiTile.svelte';

  interface Props {
    agentId: string;
    agentName: string;
    agentStatus: AgentStatus;
    onopensettings?: () => void;
  }

  let { agentId, agentName, agentStatus, onopensettings }: Props = $props();

  const TILE_TYPES = ['chat', 'tui', 'telemetry', 'trace', 'files', 'activity', 'memory', 'checkpoints'] as const;
  type TileType = typeof TILE_TYPES[number];

  const TILE_LABELS: Record<TileType, string> = {
    chat: 'Chat',
    tui: 'Claude TUI',
    telemetry: 'Telemetry',
    trace: 'Trace Log',
    files: 'Files',
    activity: 'Activity',
    memory: 'Memory',
    checkpoints: 'Checkpoints',
  };

  let activeTiles = $state<TileType[]>(['chat']);
  let maximizedTile = $state<TileType | null>(null);

  // Visible tiles: if one is maximized, show only that; otherwise show all active
  let visibleTiles = $derived(maximizedTile ? [maximizedTile] : activeTiles);

  // Grid class based on tile count
  let gridClass = $derived.by(() => {
    const count = visibleTiles.length;
    if (count <= 1) return 'grid-1';
    if (count === 2) return 'grid-2';
    if (count === 3) return 'grid-3';
    return 'grid-4';
  });

  function toggleTile(tile: TileType) {
    if (tile === 'chat') return; // chat cannot be removed
    if (activeTiles.includes(tile)) {
      activeTiles = activeTiles.filter(t => t !== tile);
      if (maximizedTile === tile) maximizedTile = null;
    } else {
      activeTiles = [...activeTiles, tile];
    }
  }

  function closeTile(tile: TileType) {
    if (tile === 'chat') return;
    activeTiles = activeTiles.filter(t => t !== tile);
    if (maximizedTile === tile) maximizedTile = null;
  }

  function toggleMaximize(tile: TileType) {
    maximizedTile = maximizedTile === tile ? null : tile;
  }

  function goBack() {
    setFocusedAgent(null);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      goBack();
    }
  }

  onMount(() => {
    window.addEventListener('keydown', handleKeydown);
  });

  onDestroy(() => {
    window.removeEventListener('keydown', handleKeydown);
  });

  // Status badge color
  let parsedStatus = $derived(parseAgentStatus(agentStatus));
  let statusLabel = $derived(statusText(agentStatus));
  let statusColor = $derived.by(() => {
    switch (parsedStatus) {
      case 'working': return 'var(--status-working, #22c55e)';
      case 'attention': return 'var(--status-waiting, #f59e0b)';
      case 'error': return 'var(--status-error, #ef4444)';
      default: return 'var(--text-muted)';
    }
  });

  // Tile split ratios (0..1, fraction for first column/row)
  let colSplit = $state(0.5);
  let rowSplit = $state(0.5);

  // Dynamic grid style based on split ratios
  let gridStyle = $derived.by(() => {
    const count = visibleTiles.length;
    if (count <= 1) return '';
    const colPct = Math.round(colSplit * 1000) / 10;
    const rowPct = Math.round(rowSplit * 1000) / 10;
    if (count === 2) return `grid-template-columns: ${colPct}% ${100 - colPct}%`;
    if (count === 3) return `grid-template-columns: ${colPct}% ${100 - colPct}%; grid-template-rows: ${rowPct}% ${100 - rowPct}%`;
    return `grid-template-columns: ${colPct}% ${100 - colPct}%; grid-template-rows: ${rowPct}% ${100 - rowPct}%`;
  });

  // Resize drag state
  let resizing = $state<'col' | 'row' | null>(null);
  let gridEl: HTMLDivElement | undefined = $state();

  function startResize(axis: 'col' | 'row', e: PointerEvent) {
    resizing = axis;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onResizeMove(e: PointerEvent) {
    if (!resizing || !gridEl) return;
    const rect = gridEl.getBoundingClientRect();
    if (resizing === 'col') {
      colSplit = Math.max(0.15, Math.min(0.85, (e.clientX - rect.left) / rect.width));
    } else {
      rowSplit = Math.max(0.15, Math.min(0.85, (e.clientY - rect.top) / rect.height));
    }
  }

  function stopResize() {
    resizing = null;
  }
</script>

<div class="workspace-view">
  <!-- Top Bar -->
  <div class="top-bar">
    <div class="top-bar-left">
      <span class="agent-name">{agentName}</span>
      <span class="status-badge" style="background: {statusColor}">
        {statusLabel}
      </span>
    </div>

    <div class="tile-dock">
      {#each TILE_TYPES as tile}
        <button
          class="dock-btn"
          class:active={activeTiles.includes(tile)}
          class:locked={tile === 'chat'}
          onclick={() => toggleTile(tile)}
          title={tile === 'chat' ? 'Chat (always active)' : `Toggle ${TILE_LABELS[tile]}`}
        >
          + {TILE_LABELS[tile]}
        </button>
      {/each}
    </div>

    <div class="top-bar-right">
      {#if onopensettings}
        <button class="header-icon-btn" onclick={onopensettings} title="Agent settings">
          &#x2699;
        </button>
      {/if}
      <button class="header-icon-btn close" onclick={goBack} title="Close workspace (Esc)">
        &#x2715;
      </button>
    </div>
  </div>

  <!-- Tile Grid -->
  <div
    class="tile-grid {gridClass}"
    role="presentation"
    class:resizing={resizing !== null}
    style={gridStyle}
    bind:this={gridEl}
    onpointermove={onResizeMove}
    onpointerup={stopResize}
  >
    {#each visibleTiles as tile, i (tile)}
      <div class="tile" class:tile-first={i === 0 && visibleTiles.length === 3 && !maximizedTile}>
        <div class="tile-header">
          <span class="tile-title">{TILE_LABELS[tile]}</span>
          <div class="tile-actions">
            <button class="tile-action" onclick={() => toggleMaximize(tile)} title={maximizedTile === tile ? 'Restore' : 'Maximize'}>
              {maximizedTile === tile ? '\u25a3' : '\u25a1'}
            </button>
            {#if tile !== 'chat'}
              <button class="tile-action close" onclick={() => closeTile(tile)} title="Close">
                &#x00D7;
              </button>
            {/if}
          </div>
        </div>
        <div class="tile-content">
          {#if tile === 'chat'}
            <ChatTile agentId={agentId} />
          {:else if tile === 'tui'}
            <ClaudeTuiTile agentId={agentId} />
          {:else if tile === 'telemetry'}
            <TelemetryTile agentId={agentId} />
          {:else if tile === 'trace'}
            <TraceTile agentId={agentId} />
          {:else if tile === 'files'}
            <FileBrowserTile agentId={agentId} />
          {:else if tile === 'activity'}
            <ActivityTile agentId={agentId} />
          {:else if tile === 'memory'}
            <MemoryTile agentId={agentId} />
          {:else if tile === 'checkpoints'}
            <CheckpointTile agentId={agentId} />
          {/if}
        </div>
      </div>
    {/each}

    <!-- Resize handles (only when 2+ tiles visible) -->
    {#if visibleTiles.length >= 2}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="resize-handle-col" onpointerdown={(e) => startResize('col', e)} style="left: {colSplit * 100}%"></div>
    {/if}
    {#if visibleTiles.length >= 3}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="resize-handle-row" onpointerdown={(e) => startResize('row', e)} style="top: {rowSplit * 100}%"></div>
    {/if}
  </div>
</div>

<style>
  .workspace-view {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    padding-bottom: 28px; /* space for the status strip footer */
  }

  /* Top Bar */
  .top-bar {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 8px 16px;
    background: var(--card-bg);
    border-bottom: var(--border-width) solid var(--surface-border);
    flex-shrink: 0;
  }

  .top-bar-left {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-shrink: 0;
  }

  .agent-name {
    font-weight: 700;
    font-size: 1.071rem;
    color: var(--text-primary);
  }

  .status-badge {
    font-size: 0.714rem;
    font-family: var(--mono-family);
    color: #000;
    padding: 2px 8px;
    border-radius: 9999px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-weight: 600;
  }

  /* Tile Dock */
  .tile-dock {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: 1;
    justify-content: center;
  }

  .dock-btn {
    font-size: 0.857rem;
    font-family: var(--mono-family);
    padding: 4px 10px;
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    background: var(--surface-border);
    color: var(--text-secondary);
    cursor: pointer;
    transition: background 120ms, color 120ms;
  }

  .dock-btn:hover {
    color: var(--text-primary);
  }

  .dock-btn.active {
    background: var(--agent-accent, #06b6d4);
    color: #000;
    border-color: var(--agent-accent, #06b6d4);
  }

  .dock-btn.locked {
    cursor: default;
    opacity: 0.8;
  }

  /* Header right buttons */
  .top-bar-right {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .header-icon-btn {
    width: 30px;
    height: 30px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
    font-size: 1rem;
    transition: color 120ms, border-color 120ms, background 120ms;
  }

  .header-icon-btn:hover {
    color: var(--text-primary);
    border-color: var(--text-muted);
  }

  .header-icon-btn.close:hover {
    color: var(--alert-accent);
    border-color: var(--alert-accent);
    background: color-mix(in srgb, var(--alert-accent) 10%, transparent);
  }

  /* Tile Grid */
  .tile-grid {
    flex: 1;
    display: grid;
    gap: 1px;
    background: var(--surface-border);
    overflow: hidden;
    position: relative;
  }

  .tile-grid.resizing {
    cursor: col-resize;
    user-select: none;
  }

  .tile-grid.grid-1 {
    grid-template-columns: 1fr;
  }

  /* grid-2, grid-3, grid-4 use dynamic inline styles for columns/rows */
  .tile-grid.grid-2 {
    grid-template-columns: 1fr 1fr; /* fallback, overridden by inline style */
  }

  .tile-grid.grid-3 {
    grid-template-columns: 1fr 1fr;
    grid-template-rows: 1fr 1fr;
  }

  .tile-grid.grid-3 .tile-first {
    grid-column: 1 / -1;
  }

  .tile-grid.grid-4 {
    grid-template-columns: 1fr 1fr;
    grid-template-rows: 1fr 1fr;
  }

  /* Resize handles */
  .resize-handle-col, .resize-handle-row {
    position: absolute;
    z-index: 5;
    background: transparent;
    transition: background 0.15s;
  }

  .resize-handle-col {
    top: 0;
    bottom: 0;
    width: 6px;
    transform: translateX(-50%);
    cursor: col-resize;
  }

  .resize-handle-row {
    left: 0;
    right: 0;
    height: 6px;
    transform: translateY(-50%);
    cursor: row-resize;
  }

  .resize-handle-col:hover, .resize-handle-row:hover {
    background: var(--agent-accent, #06b6d4);
    opacity: 0.5;
  }

  /* Tile */
  .tile {
    display: flex;
    flex-direction: column;
    background: var(--card-bg);
    overflow: hidden;
  }

  .tile-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 10px;
    border-bottom: var(--border-width) solid var(--surface-border);
    flex-shrink: 0;
  }

  .tile-title {
    font-size: 0.786rem;
    font-family: var(--mono-family);
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-weight: 600;
  }

  .tile-actions {
    display: flex;
    align-items: center;
    gap: 2px;
  }

  .tile-action {
    width: 22px;
    height: 22px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: none;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    border-radius: 3px;
    font-size: 1rem;
    line-height: 1;
  }

  .tile-action:hover {
    background: var(--surface-border);
    color: var(--text-primary);
  }

  .tile-action.close:hover {
    background: var(--status-error, #ef4444);
    color: #fff;
  }

  .tile-content {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .tile-placeholder {
    font-family: var(--mono-family);
    font-size: 0.929rem;
    color: var(--text-muted);
    opacity: 0.5;
  }
</style>
