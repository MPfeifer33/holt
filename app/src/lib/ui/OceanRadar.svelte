<script lang="ts">
  import type { OceanScores } from '$lib/tauri/commands';

  interface Props {
    scores: OceanScores;
    /** Overlay scores for visual diff (e.g. base vs base+modifiers) */
    overlay?: OceanScores;
    /** Size in pixels (width & height) */
    size?: number;
    /** Accent color for the primary polygon */
    color?: string;
    /** Accent color for the overlay polygon */
    overlayColor?: string;
    /** Show axis labels */
    labels?: boolean;
    /** Enable interactive mode — data points become draggable */
    interactive?: boolean;
    /** Callback when a trait is changed via drag (interactive mode only) */
    onchange?: (trait: keyof OceanScores, value: number) => void;
  }

  let {
    scores,
    overlay,
    size = 120,
    color = 'var(--infra-accent)',
    overlayColor = 'var(--text-muted)',
    labels = true,
    interactive = false,
    onchange,
  }: Props = $props();

  // Pentagon geometry: 5 axes, starting from top (270deg), clockwise
  // O=top, C=top-right, E=bottom-right, A=bottom-left, N=top-left
  const AXES = ['O', 'C', 'E', 'A', 'N'] as const;
  const AXIS_KEYS: (keyof OceanScores)[] = [
    'openness',
    'conscientiousness',
    'extraversion',
    'agreeableness',
    'neuroticism',
  ];

  // Full trait names for tooltips
  const AXIS_NAMES: Record<string, string> = {
    O: 'Openness',
    C: 'Conscientiousness',
    E: 'Extraversion',
    A: 'Agreeableness',
    N: 'Neuroticism',
  };

  const cx = 50;
  const cy = 50;
  const radius = 38;
  const labelRadius = 47;

  function pointOnAxis(axisIndex: number, value: number): { x: number; y: number } {
    const angle = ((2 * Math.PI) / 5) * axisIndex - Math.PI / 2;
    return {
      x: cx + radius * value * Math.cos(angle),
      y: cy + radius * value * Math.sin(angle),
    };
  }

  function labelPosition(axisIndex: number): { x: number; y: number } {
    const angle = ((2 * Math.PI) / 5) * axisIndex - Math.PI / 2;
    return {
      x: cx + labelRadius * Math.cos(angle),
      y: cy + labelRadius * Math.sin(angle),
    };
  }

  function polygonPoints(s: OceanScores): string {
    return AXIS_KEYS.map((key, i) => {
      const pt = pointOnAxis(i, s[key]);
      return `${pt.x},${pt.y}`;
    }).join(' ');
  }

  function ringPoints(value: number): string {
    return Array.from({ length: 5 }, (_, i) => {
      const pt = pointOnAxis(i, value);
      return `${pt.x},${pt.y}`;
    }).join(' ');
  }

  let primaryPoints = $derived(polygonPoints(scores));
  let overlayPoints = $derived(overlay ? polygonPoints(overlay) : '');

  // ── Interactive drag handling ──────────────────────────────────────

  let draggingIndex: number | null = $state(null);
  let svgEl: SVGSVGElement | undefined = $state(undefined);
  /** Track which traits were manually adjusted (for visual distinction) */
  let manuallyAdjusted = $state<Set<number>>(new Set());

  function svgPoint(clientX: number, clientY: number): { x: number; y: number } {
    if (!svgEl) return { x: cx, y: cy };
    const rect = svgEl.getBoundingClientRect();
    const scaleX = 100 / rect.width;
    const scaleY = 100 / rect.height;
    return {
      x: (clientX - rect.left) * scaleX,
      y: (clientY - rect.top) * scaleY,
    };
  }

  function valueFromPosition(axisIndex: number, svgX: number, svgY: number): number {
    const angle = ((2 * Math.PI) / 5) * axisIndex - Math.PI / 2;
    const dx = svgX - cx;
    const dy = svgY - cy;
    // Project onto axis direction
    const projection = dx * Math.cos(angle) + dy * Math.sin(angle);
    return Math.max(0, Math.min(1, projection / radius));
  }

  function handlePointerDown(axisIndex: number) {
    return (e: PointerEvent) => {
      if (!interactive) return;
      draggingIndex = axisIndex;
      manuallyAdjusted.add(axisIndex);
      (e.target as Element)?.setPointerCapture?.(e.pointerId);
      e.preventDefault();
    };
  }

  function handlePointerMove(e: PointerEvent) {
    if (draggingIndex === null || !interactive) return;
    const pt = svgPoint(e.clientX, e.clientY);
    const value = valueFromPosition(draggingIndex, pt.x, pt.y);
    const rounded = Math.round(value * 100) / 100;
    onchange?.(AXIS_KEYS[draggingIndex], rounded);
  }

  function handlePointerUp() {
    draggingIndex = null;
  }

  function handleDoubleClick(axisIndex: number) {
    // Double-click resets manual override (remove from adjusted set)
    if (!interactive) return;
    manuallyAdjusted.delete(axisIndex);
    manuallyAdjusted = new Set(manuallyAdjusted); // trigger reactivity
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<svg
  bind:this={svgEl}
  width={size}
  height={size}
  viewBox="0 0 100 100"
  class="ocean-radar"
  class:interactive
  aria-label="OCEAN personality radar chart"
  onpointermove={handlePointerMove}
  onpointerup={handlePointerUp}
  onpointerleave={handlePointerUp}
>
  <!-- Grid rings -->
  {#each [0.25, 0.5, 0.75, 1.0] as ring}
    <polygon
      points={ringPoints(ring)}
      fill="none"
      stroke="var(--surface-border)"
      stroke-width="0.5"
      opacity={ring === 1.0 ? 0.6 : 0.3}
    />
  {/each}

  <!-- Axis lines -->
  {#each AXIS_KEYS as _, i}
    {@const pt = pointOnAxis(i, 1.0)}
    <line
      x1={cx}
      y1={cy}
      x2={pt.x}
      y2={pt.y}
      stroke="var(--surface-border)"
      stroke-width="0.4"
      opacity="0.4"
    />
  {/each}

  <!-- Overlay polygon (if present) -->
  {#if overlay}
    <polygon
      points={overlayPoints}
      fill={overlayColor}
      fill-opacity="0.08"
      stroke={overlayColor}
      stroke-width="0.8"
      stroke-opacity="0.4"
      stroke-dasharray="2 1.5"
    />
  {/if}

  <!-- Primary polygon -->
  <polygon
    points={primaryPoints}
    fill={color}
    fill-opacity="0.15"
    stroke={color}
    stroke-width="1.2"
    stroke-linejoin="round"
  />

  <!-- Data points -->
  {#each AXIS_KEYS as key, i}
    {@const pt = pointOnAxis(i, scores[key])}
    {@const isOverridden = manuallyAdjusted.has(i)}
    {@const isDragging = draggingIndex === i}
    {#if interactive}
      <!-- Interactive: larger hit target, visual feedback -->
      <circle
        cx={pt.x}
        cy={pt.y}
        r={isDragging ? 3.5 : 2.5}
        fill={isOverridden ? 'transparent' : color}
        stroke={color}
        stroke-width={isOverridden ? 1.2 : 0}
        class="draggable-point"
        onpointerdown={handlePointerDown(i)}
        ondblclick={() => handleDoubleClick(i)}
        role="slider"
        aria-label="{AXIS_NAMES[AXES[i]]}: {(scores[key] * 100).toFixed(0)}%"
        aria-valuenow={scores[key] * 100}
        aria-valuemin={0}
        aria-valuemax={100}
        tabindex={0}
      >
        <title>{AXIS_NAMES[AXES[i]]}: {(scores[key] * 100).toFixed(0)}%{isOverridden ? ' (manually adjusted — double-click to reset)' : ''}</title>
      </circle>
      <!-- Invisible larger hit area for easier grabbing -->
      <circle
        cx={pt.x}
        cy={pt.y}
        r="5"
        fill="transparent"
        class="hit-area"
        onpointerdown={handlePointerDown(i)}
        ondblclick={() => handleDoubleClick(i)}
      />
    {:else}
      <circle cx={pt.x} cy={pt.y} r="1.8" fill={color} />
    {/if}
  {/each}

  <!-- Axis labels -->
  {#if labels}
    {#each AXES as label, i}
      {@const pos = labelPosition(i)}
      <text
        x={pos.x}
        y={pos.y}
        text-anchor="middle"
        dominant-baseline="central"
        font-size="7"
        font-weight="600"
        fill="var(--text-secondary)"
        font-family="var(--font-family)"
      >
        {label}
      </text>
    {/each}
  {/if}
</svg>

<style>
  .ocean-radar {
    display: block;
    flex-shrink: 0;
  }

  .ocean-radar.interactive {
    cursor: default;
  }

  .draggable-point {
    cursor: grab;
    transition: r 0.1s ease;
  }

  .draggable-point:hover {
    r: 3;
  }

  .hit-area {
    cursor: grab;
  }
</style>
