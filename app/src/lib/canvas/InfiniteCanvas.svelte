<script lang="ts">
  import { getCamera, panBy, zoomTo, setCamera, setIsPanning, resetCamera, getZoom } from '$lib/stores/canvas.svelte';
  import { ZOOM_SENSITIVITY, ZOOM_MIN, ZOOM_MAX, GRID_BASE_SIZE } from '$lib/constants';
  import type { Snippet } from 'svelte';

  let {
    children,
    onshowcontextmenu,
  }: {
    children?: Snippet;
    onshowcontextmenu?: (x: number, y: number, target: 'canvas') => void;
  } = $props();

  let canvasEl: HTMLDivElement;
  let dragStart: { x: number; y: number } | null = $state(null);

  function handlePointerDown(e: PointerEvent) {
    // Only pan on left-click or middle-click on empty canvas
    if (e.button !== 0 && e.button !== 1) return;
    if (e.target !== e.currentTarget) return;

    dragStart = { x: e.clientX, y: e.clientY };
    setIsPanning(true);
    canvasEl.setPointerCapture(e.pointerId);
  }

  function handlePointerMove(e: PointerEvent) {
    if (!dragStart) return;

    const zoom = getZoom();
    const dx = (e.clientX - dragStart.x) / zoom;
    const dy = (e.clientY - dragStart.y) / zoom;
    panBy(dx, dy);
    dragStart = { x: e.clientX, y: e.clientY };
  }

  function handlePointerUp(e: PointerEvent) {
    if (!dragStart) return;
    dragStart = null;
    setIsPanning(false);
    canvasEl.releasePointerCapture(e.pointerId);
  }

  function handleWheel(e: WheelEvent) {
    e.preventDefault();

    const c = getCamera();
    const rect = canvasEl.getBoundingClientRect();

    // Mouse position relative to the canvas element
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;

    // World position under cursor before zoom
    const wx = (mx - rect.width / 2) / c.zoom - c.x;
    const wy = (my - rect.height / 2) / c.zoom - c.y;

    // Compute new zoom
    const zoomDelta = -e.deltaY * ZOOM_SENSITIVITY;
    const newZoom = Math.min(Math.max(ZOOM_MIN, c.zoom + zoomDelta), ZOOM_MAX);

    // Adjust pan so the world point under cursor stays fixed
    const newX = (mx - rect.width / 2) / newZoom - wx;
    const newY = (my - rect.height / 2) / newZoom - wy;

    setCamera({ x: newX, y: newY, zoom: newZoom });
  }

  function handleContextMenu(e: MouseEvent) {
    // Only fire on empty canvas space, not on child nodes
    if (e.target !== e.currentTarget) return;
    e.preventDefault();
    onshowcontextmenu?.(e.clientX, e.clientY, 'canvas');
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (!e.ctrlKey && !e.metaKey) return;

    if (e.key === '=' || e.key === '+') {
      e.preventDefault();
      const c = getCamera();
      zoomTo(c.zoom + 0.1);
    } else if (e.key === '-') {
      e.preventDefault();
      const c = getCamera();
      zoomTo(c.zoom - 0.1);
    } else if (e.key === '0') {
      e.preventDefault();
      resetCamera();
    }
  }

  // Reactive derivations for style
  let cam = $derived(getCamera());
  let gridSize = $derived(GRID_BASE_SIZE * cam.zoom);
  let gridOffsetX = $derived((cam.x * cam.zoom) % gridSize);
  let gridOffsetY = $derived((cam.y * cam.zoom) % gridSize);
  let dotRadius = $derived(Math.max(0.5, 0.8 * cam.zoom));

  let gridStyle = $derived(
    `background-image: radial-gradient(circle, var(--grid-dot) ${dotRadius}px, transparent ${dotRadius}px); ` +
    `background-size: ${gridSize}px ${gridSize}px; ` +
    `background-position: ${gridOffsetX}px ${gridOffsetY}px;`
  );

  let contentTransform = $derived(
    `translate(${cam.x * cam.zoom}px, ${cam.y * cam.zoom}px) scale(${cam.zoom})`
  );
</script>

<svelte:window onkeydown={handleKeyDown} />

<div
  class="infinite-canvas"
  bind:this={canvasEl}
  style={gridStyle}
  role="application"
  tabindex="-1"
  onpointerdown={handlePointerDown}
  onpointermove={handlePointerMove}
  onpointerup={handlePointerUp}
  onwheel={handleWheel}
  oncontextmenu={handleContextMenu}
>
  <div class="canvas-atmosphere"></div>
  <div class="content-layer" style="transform: {contentTransform}; transform-origin: center center;">
    {#if children}
      {@render children()}
    {/if}
  </div>
</div>

<style>
  .infinite-canvas {
    width: 100%;
    height: 100%;
    position: relative;
    overflow: hidden;
    cursor: grab;
    touch-action: none;
    --grid-dot: var(--text-muted, rgba(255, 255, 255, 0.08));
  }

  .infinite-canvas:active {
    cursor: grabbing;
  }

  /* Atmosphere: radial vignette + subtle gradient wash */
  .canvas-atmosphere {
    position: absolute;
    inset: 0;
    pointer-events: none;
    z-index: 0;
    background:
      radial-gradient(
        ellipse 80% 70% at 50% 50%,
        color-mix(in srgb, var(--infra-accent, #6366f1) 3%, transparent) 0%,
        transparent 70%
      ),
      radial-gradient(
        ellipse 60% 50% at 25% 30%,
        color-mix(in srgb, var(--agent-accent, #06b6d4) 2%, transparent) 0%,
        transparent 60%
      ),
      radial-gradient(
        ellipse at 50% 50%,
        transparent 40%,
        rgba(0, 0, 0, 0.3) 100%
      );
  }

  .content-layer {
    position: absolute;
    top: 50%;
    left: 50%;
    pointer-events: none;
    z-index: 1;
  }

  .content-layer > :global(*) {
    pointer-events: auto;
  }
</style>
