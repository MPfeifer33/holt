<script lang="ts">
  import type { Snippet } from 'svelte';
  import { updateWindowPosition, updateWindowSize, focusWindow } from '$lib/stores/windows.svelte';

  interface Props {
    agentId: string;
    title: string;
    subtitle?: string;
    avatar?: string;
    statusMessage?: string;
    statusColor?: string;
    statusGlow?: boolean;
    accentColor?: string;
    glowColor?: string;
    x: number;
    y: number;
    width: number;
    height: number;
    zIndex: number;
    hidden?: boolean;
    onclose: () => void;
    onminimize: () => void;
    children: Snippet;
  }

  let {
    agentId, title, subtitle, avatar, statusMessage,
    statusColor = 'var(--text-muted)', statusGlow = false,
    accentColor, glowColor,
    x, y, width, height, zIndex, hidden = false,
    onclose, onminimize, children,
  }: Props = $props();

  const MIN_W = 280;
  const MIN_H = 200;

  let dragging = $state(false);
  let resizing = $state<string | null>(null);
  let dragOffset = { x: 0, y: 0 };
  let resizeStart = { x: 0, y: 0, w: 0, h: 0, px: 0, py: 0 };

  function onDragStart(e: PointerEvent) {
    if (e.button !== 0) return;
    dragging = true;
    dragOffset = { x: e.clientX - x, y: e.clientY - y };
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    focusWindow(agentId);
  }

  let dragRAF: number | null = null;
  function onDragMove(e: PointerEvent) {
    if (!dragging) return;
    if (dragRAF) return;
    dragRAF = requestAnimationFrame(() => {
      dragRAF = null;
      if (!dragging) return;
      const nx = e.clientX - dragOffset.x;
      const ny = e.clientY - dragOffset.y;
      updateWindowPosition(agentId, Math.max(0, nx), Math.max(0, ny));
    });
  }

  function onDragEnd(e: PointerEvent) {
    dragging = false;
    const el = e.target as HTMLElement;
    if (el.hasPointerCapture?.(e.pointerId)) el.releasePointerCapture(e.pointerId);
  }

  function onResizeStart(e: PointerEvent, edge: string) {
    if (e.button !== 0) return;
    e.stopPropagation();
    resizing = edge;
    resizeStart = { x, y, w: width, h: height, px: e.clientX, py: e.clientY };
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    focusWindow(agentId);
  }

  let resizeRAF: number | null = null;
  function onResizeMove(e: PointerEvent) {
    if (!resizing) return;
    if (resizeRAF) return;
    resizeRAF = requestAnimationFrame(() => {
      resizeRAF = null;
      if (!resizing) return;
      const dx = e.clientX - resizeStart.px;
      const dy = e.clientY - resizeStart.py;
      let nx = resizeStart.x, ny = resizeStart.y, nw = resizeStart.w, nh = resizeStart.h;

      if (resizing.includes('e')) nw = Math.max(MIN_W, resizeStart.w + dx);
      if (resizing.includes('s')) nh = Math.max(MIN_H, resizeStart.h + dy);
      if (resizing.includes('w')) {
        const newW = Math.max(MIN_W, resizeStart.w - dx);
        nx = resizeStart.x + (resizeStart.w - newW);
        nw = newW;
      }
      if (resizing.includes('n')) {
        const newH = Math.max(MIN_H, resizeStart.h - dy);
        ny = resizeStart.y + (resizeStart.h - newH);
        nh = newH;
      }

      updateWindowPosition(agentId, Math.max(0, nx), Math.max(0, ny));
      updateWindowSize(agentId, nw, nh);
    });
  }

  function onResizeEnd(e: PointerEvent) {
    resizing = null;
    const el = e.target as HTMLElement;
    if (el.hasPointerCapture?.(e.pointerId)) el.releasePointerCapture(e.pointerId);
  }

  function handleFocus() {
    focusWindow(agentId);
  }

  let effectiveGlow = $derived(glowColor || 'var(--glow-color)');
  let effectiveAccent = $derived(accentColor || statusColor);
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="floating-panel"
  style="left:{x}px; top:{y}px; width:{width}px; height:{height}px; z-index:{zIndex};
         --panel-glow: {effectiveGlow}; --panel-accent: {effectiveAccent};
         box-shadow: var(--card-shadow, 0 4px 20px rgba(0,0,0,0.3)), 0 0 calc(20px * var(--glow-intensity, 0.5)) color-mix(in srgb, var(--panel-glow) calc(var(--glow-intensity, 0.5) * 30%), transparent);
         {hidden ? 'display: none;' : ''}"
  onpointerdown={handleFocus}
>
  <!-- Resize handles -->
  {#each ['n','s','e','w','ne','nw','se','sw'] as edge}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="resize-handle resize-{edge}"
      onpointerdown={(e) => onResizeStart(e, edge)}
      onpointermove={onResizeMove}
      onpointerup={onResizeEnd}
      onpointercancel={onResizeEnd}
    ></div>
  {/each}

  <!-- Header -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="panel-header"
    onpointerdown={onDragStart}
    onpointermove={onDragMove}
    onpointerup={onDragEnd}
    onpointercancel={onDragEnd}
  >
    <div class="header-left">
      {#if avatar}
        <span class="avatar">{avatar}</span>
      {/if}
      <div class="status-dot" style="background: {effectiveAccent};"
        class:glow={statusGlow}
        style:--dot-color={effectiveAccent}
      ></div>
      <div class="header-info">
        <span class="header-title">{title}</span>
        {#if subtitle}
          <span class="header-subtitle">{subtitle}</span>
        {/if}
      </div>
    </div>
    <div class="header-right">
      {#if statusMessage}
        <span class="status-message">{statusMessage}</span>
      {/if}
      <button class="header-btn" onclick={onminimize} title="Minimize">_</button>
      <button class="header-btn" onclick={onclose} title="Close">&times;</button>
    </div>
  </div>

  <!-- Body -->
  <div class="panel-body">
    {@render children()}
  </div>
</div>

<style>
  .floating-panel {
    position: fixed;
    display: flex;
    flex-direction: column;
    background: color-mix(in srgb, var(--card-bg) calc(var(--surface-opacity, 1) * 100%), transparent);
    backdrop-filter: blur(var(--backdrop-blur, 0px));
    border: var(--border-width, 1px) var(--border-style, solid)
      color-mix(in srgb, var(--surface-border) calc(var(--border-opacity, 1) * 100%), transparent);
    border-radius: calc(var(--border-radius, 8px) + 2px);
    overflow: hidden;
  }

  .panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    border-bottom: 1px solid var(--surface-border);
    background: color-mix(in srgb, var(--panel-bg) 50%, transparent);
    cursor: grab;
    user-select: none;
    flex-shrink: 0;
  }

  .panel-header:active { cursor: grabbing; }

  .header-left {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .avatar {
    font-size: 1.143rem;
    flex-shrink: 0;
  }

  .status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
    transition: box-shadow var(--transition-speed, 200ms) ease;
  }

  .status-dot.glow {
    box-shadow: 0 0 6px var(--dot-color);
  }

  .header-info {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .header-title {
    font-size: 1rem;
    font-weight: var(--heading-weight, 600);
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .header-subtitle {
    font-size: 0.786rem;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .header-right {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .status-message {
    font-size: 0.786rem;
    color: var(--text-muted);
    font-style: italic;
    max-width: 150px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .header-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    padding: 2px 6px;
    font-size: 0.857rem;
    border-radius: 4px;
    transition: color var(--transition-speed, 200ms) ease, background var(--transition-speed, 200ms) ease;
  }

  .header-btn:hover {
    color: var(--text-primary);
    background: color-mix(in srgb, var(--surface-border) 30%, transparent);
  }

  .panel-body {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .resize-handle { position: absolute; z-index: 10; }
  .resize-n { top: -3px; left: 8px; right: 8px; height: 6px; cursor: n-resize; }
  .resize-s { bottom: -3px; left: 8px; right: 8px; height: 6px; cursor: s-resize; }
  .resize-e { right: -3px; top: 8px; bottom: 8px; width: 6px; cursor: e-resize; }
  .resize-w { left: -3px; top: 8px; bottom: 8px; width: 6px; cursor: w-resize; }
  .resize-ne { top: -3px; right: -3px; width: 12px; height: 12px; cursor: ne-resize; }
  .resize-nw { top: -3px; left: -3px; width: 12px; height: 12px; cursor: nw-resize; }
  .resize-se { bottom: -3px; right: -3px; width: 12px; height: 12px; cursor: se-resize; }
  .resize-sw { bottom: -3px; left: -3px; width: 12px; height: 12px; cursor: sw-resize; }
</style>
