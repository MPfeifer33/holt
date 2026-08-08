<script lang="ts">
  import { isGraphView } from '$lib/stores/canvas.svelte';
  import { SATELLITE_RADIUS, SATELLITE_SIZE, SATELLITE_ARC_SPREAD, TRUNC_SUBAGENT_NAME } from '$lib/constants';

  let {
    parentX,
    parentY,
    parentW,
    parentH,
    subagents,
  }: {
    parentX: number;
    parentY: number;
    parentW: number;
    parentH: number;
    subagents: Array<{ id: string; name: string; status: string }>;
  } = $props();

  let graphView = $derived(isGraphView());

  // Status color matching AgentNode scheme
  function statusColor(status: string): string {
    if (status === 'Working' || status === 'running') return 'var(--agent-accent)';
    if (status === 'WaitingForHil' || status === 'blocked') return 'var(--alert-accent)';
    if (status === 'done' || status === 'complete' || status === 'Idle') return 'var(--surface-border)';
    return 'var(--surface-border)';
  }

  // Arc positions to the right of the parent circle
  // Spread subagents in a small arc from ~-40° to +40° off the right edge
  const ARC_SPREAD_RAD = SATELLITE_ARC_SPREAD;

  function satellitePosition(index: number, total: number): { x: number; y: number } {
    // Distribute evenly across the arc; single item → straight right
    const startAngle = -ARC_SPREAD_RAD / 2;
    const step = total > 1 ? ARC_SPREAD_RAD / (total - 1) : 0;
    const angle = startAngle + index * step;

    // Parent center is at (parentX, parentY) in graph view
    // Place satellites to the right: base angle = 0 (pointing right)
    const cx = parentX + (parentW / 2) * 1.0 + Math.cos(angle) * SATELLITE_RADIUS;
    const cy = parentY + Math.sin(angle) * SATELLITE_RADIUS;
    return { x: cx, y: cy };
  }

  // Derive satellite layout
  let satellites = $derived(
    subagents.map((sa, i) => {
      const pos = satellitePosition(i, subagents.length);
      return { ...sa, ...pos, color: statusColor(sa.status) };
    })
  );
</script>

{#if graphView && subagents.length > 0}
  <!-- Rendered as SVG-adjacent absolutely-positioned elements inside the canvas world -->
  <!-- We use a <svg> fragment that the parent must embed inside the connections-overlay SVG -->
  <!-- Since AgentNode renders as HTML divs, SubagentCluster renders as its own positioned div -->
  <div class="subagent-cluster" style="left: {parentX}px; top: {parentY}px;">
    <svg class="cluster-svg" overflow="visible">
      {#each satellites as sat (sat.id)}
        {@const lineEndX = sat.x - parentX}
        {@const lineEndY = sat.y - parentY}
        <!-- Dashed connector line from parent edge to satellite -->
        <line
          x1={parentW / 2}
          y1={0}
          x2={lineEndX}
          y2={lineEndY}
          stroke="var(--surface-border)"
          stroke-width="1"
          stroke-dasharray="4 3"
          opacity="0.6"
        />
        <!-- Satellite circle -->
        <circle
          cx={lineEndX}
          cy={lineEndY}
          r={SATELLITE_SIZE / 2}
          fill="var(--card-bg)"
          stroke={sat.color}
          stroke-width="2"
        />
        <!-- Status dot center -->
        <circle
          cx={lineEndX}
          cy={lineEndY}
          r={4}
          fill={sat.color}
          opacity="0.8"
        />
        <!-- Name label below satellite -->
        <text
          x={lineEndX}
          y={lineEndY + SATELLITE_SIZE / 2 + 12}
          text-anchor="middle"
          font-size="9"
          fill="var(--text-muted)"
          font-family="var(--mono-family)"
        >{sat.name.length > TRUNC_SUBAGENT_NAME + 1 ? sat.name.slice(0, TRUNC_SUBAGENT_NAME) + '\u2026' : sat.name}</text>
      {/each}
    </svg>
  </div>
{/if}

<style>
  .subagent-cluster {
    position: absolute;
    /* zero-size container; content is in SVG with overflow visible */
    width: 0;
    height: 0;
    pointer-events: none;
  }

  .cluster-svg {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: visible;
  }
</style>
