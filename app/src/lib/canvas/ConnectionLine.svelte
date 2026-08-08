<script lang="ts">
  import type { Connection } from '$lib/stores/agents.svelte';
  import { isGraphView } from '$lib/stores/canvas.svelte';

  let {
    connection,
    sourceX,
    sourceY,
    sourceW,
    sourceH,
    targetX,
    targetY,
    targetW,
    targetH,
    onshowcontextmenu,
  }: {
    connection: Connection;
    sourceX: number;
    sourceY: number;
    sourceW: number;
    sourceH: number;
    targetX: number;
    targetY: number;
    targetW: number;
    targetH: number;
    onshowcontextmenu?: (x: number, y: number, connectionId: string) => void;
  } = $props();

  let graphView = $derived(isGraphView());

  function handleContextMenu(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    onshowcontextmenu?.(e.clientX, e.clientY, connection.id);
  }

  // Calculate edge attachment points based on relative node positions.
  // Nodes are centered (transform: translate(-50%, -50%)) so sourceX/Y are center coords.
  // Pick the exit/entry edges that face each other for clean routing.
  let endpoints = $derived.by(() => {
    const dx = targetX - sourceX;
    const dy = targetY - sourceY;
    const absDx = Math.abs(dx);
    const absDy = Math.abs(dy);

    let sx: number, sy: number, ex: number, ey: number;
    // Direction vectors for control points (outward from edge)
    let sdx: number, sdy: number, edx: number, edy: number;

    if (absDx >= absDy) {
      // Horizontal-dominant: exit right/left edges
      if (dx >= 0) {
        // Target is to the right
        sx = sourceX + sourceW / 2; sy = sourceY;
        ex = targetX - targetW / 2; ey = targetY;
        sdx = 1; sdy = 0; edx = -1; edy = 0;
      } else {
        // Target is to the left
        sx = sourceX - sourceW / 2; sy = sourceY;
        ex = targetX + targetW / 2; ey = targetY;
        sdx = -1; sdy = 0; edx = 1; edy = 0;
      }
    } else {
      // Vertical-dominant: exit top/bottom edges
      if (dy >= 0) {
        // Target is below
        sx = sourceX; sy = sourceY + sourceH / 2;
        ex = targetX; ey = targetY - targetH / 2;
        sdx = 0; sdy = 1; edx = 0; edy = -1;
      } else {
        // Target is above
        sx = sourceX; sy = sourceY - sourceH / 2;
        ex = targetX; ey = targetY + targetH / 2;
        sdx = 0; sdy = -1; edx = 0; edy = 1;
      }
    }

    const dist = Math.sqrt((ex - sx) ** 2 + (ey - sy) ** 2);
    const cp = Math.min(150, dist * 0.35);

    return { sx, sy, ex, ey, sdx, sdy, edx, edy, cp };
  });

  let pathD = $derived.by(() => {
    const { sx, sy, ex, ey, sdx, sdy, edx, edy, cp } = endpoints;
    return `M ${sx} ${sy} C ${sx + sdx * cp} ${sy + sdy * cp}, ${ex + edx * cp} ${ey + edy * cp}, ${ex} ${ey}`;
  });

  // Subagent lines only visible at graph view
  let visible = $derived(connection.type !== 'subagent' || graphView);
</script>

{#if visible}
  <g class="connection-line" class:active={connection.active} data-type={connection.type}>
    <!-- Hit-test area (invisible, wider stroke) -->
    <path
      d={pathD}
      fill="none"
      stroke="transparent"
      stroke-width="12"
      class="hit-area"
      role="button"
      tabindex="0"
      aria-label="Connection {connection.id}"
      oncontextmenu={handleContextMenu}
    />

    <!-- A2A connection -->
    {#if connection.type === 'a2a'}
      <path
        d={pathD}
        fill="none"
        stroke="var(--agent-accent)"
        stroke-width="1.5"
        stroke-dasharray="6 4"
        opacity={connection.active ? 0.7 : 0.3}
        class:a2a-flowing={connection.active}
      />

    <!-- Subagent connection -->
    {:else if connection.type === 'subagent'}
      <path
        d={pathD}
        fill="none"
        stroke="var(--text-muted)"
        stroke-width="1"
        stroke-dasharray="4 6"
        opacity="0.25"
      />
    {/if}
  </g>
{/if}

<style>
  .hit-area {
    pointer-events: stroke;
    cursor: pointer;
  }

  .a2a-flowing {
    animation: dash-flow 0.6s linear infinite;
  }

  @keyframes dash-flow {
    to {
      stroke-dashoffset: -20px;
    }
  }
</style>
