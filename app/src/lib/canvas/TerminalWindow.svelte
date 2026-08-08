<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { Terminal } from 'xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { WebLinksAddon } from '@xterm/addon-web-links';
  import 'xterm/css/xterm.css';

  // ---------------------------------------------------------------------------
  // Props
  // ---------------------------------------------------------------------------

  let {
    onclose,
    onminimize,
    initialX = Math.round(window.innerWidth / 2 - 360),
    initialY = Math.round(window.innerHeight / 2 - 240),
  }: {
    onclose: () => void;
    onminimize: () => void;
    initialX?: number;
    initialY?: number;
  } = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  let terminalEl: HTMLDivElement | undefined = $state();
  let sessionId: string | null = $state(null);
  let alive = $state(true);
  let terminal: Terminal | undefined;
  let fitAddon: FitAddon | undefined;
  let unlistenOutput: UnlistenFn | undefined;
  let unlistenExit: UnlistenFn | undefined;
  let resizeObserver: ResizeObserver | undefined;

  // Position & size state (snapshot prop values to avoid Svelte reactivity warning)
  // svelte-ignore state_referenced_locally
  const startX = initialX;
  // svelte-ignore state_referenced_locally
  const startY = initialY;
  let x = $state(startX);
  let y = $state(startY);
  let width = $state(720);
  let height = $state(480);

  const MIN_W = 400;
  const MIN_H = 280;

  // Drag state
  let dragging = $state(false);
  let dragOffsetX = 0;
  let dragOffsetY = 0;

  // Resize state
  let resizing = $state(false);
  let resizeEdge = '';
  let resizeStartX = 0;
  let resizeStartY = 0;
  let resizeStartW = 0;
  let resizeStartH = 0;
  let resizeStartPosX = 0;
  let resizeStartPosY = 0;

  // ---------------------------------------------------------------------------
  // Drag & Resize Handlers
  // ---------------------------------------------------------------------------

  function onHeaderPointerDown(e: PointerEvent) {
    if ((e.target as HTMLElement).closest('.header-actions')) return;
    dragging = true;
    dragOffsetX = e.clientX - x;
    dragOffsetY = e.clientY - y;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onHeaderPointerMove(e: PointerEvent) {
    if (!dragging) return;
    x = Math.max(0, Math.min(window.innerWidth - 100, e.clientX - dragOffsetX));
    y = Math.max(0, Math.min(window.innerHeight - 40, e.clientY - dragOffsetY));
  }

  function onHeaderPointerUp(e: PointerEvent) {
    dragging = false;
    (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
  }

  function onResizePointerDown(e: PointerEvent, edge: string) {
    e.stopPropagation();
    resizing = true;
    resizeEdge = edge;
    resizeStartX = e.clientX;
    resizeStartY = e.clientY;
    resizeStartW = width;
    resizeStartH = height;
    resizeStartPosX = x;
    resizeStartPosY = y;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onResizePointerMove(e: PointerEvent) {
    if (!resizing) return;
    const dx = e.clientX - resizeStartX;
    const dy = e.clientY - resizeStartY;

    if (resizeEdge.includes('e')) {
      width = Math.max(MIN_W, resizeStartW + dx);
    }
    if (resizeEdge.includes('w')) {
      const newW = Math.max(MIN_W, resizeStartW - dx);
      x = resizeStartPosX + (resizeStartW - newW);
      width = newW;
    }
    if (resizeEdge.includes('s')) {
      height = Math.max(MIN_H, resizeStartH + dy);
    }
    if (resizeEdge.includes('n')) {
      const newH = Math.max(MIN_H, resizeStartH - dy);
      y = resizeStartPosY + (resizeStartH - newH);
      height = newH;
    }

    // Refit terminal on resize
    requestAnimationFrame(() => fitAddon?.fit());
  }

  function onResizePointerUp(e: PointerEvent) {
    resizing = false;
    (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
    requestAnimationFrame(() => fitAddon?.fit());
  }

  // ---------------------------------------------------------------------------
  // Terminal Lifecycle
  // ---------------------------------------------------------------------------

  onMount(() => {
    initTerminal();
    return () => { cleanup(); };
  });

  async function initTerminal() {
    terminal = new Terminal({
      fontFamily: '"JetBrains Mono", "Fira Code", "Cascadia Code", monospace',
      fontSize: 14,
      theme: {
        background: '#0a0e14',
        foreground: '#e6e6e6',
        cursor: '#06b6d4',
        selectionBackground: '#264f78',
        black: '#0a0e14',
        red: '#ef4444',
        green: '#22c55e',
        yellow: '#f59e0b',
        blue: '#3b82f6',
        magenta: '#a78bfa',
        cyan: '#06b6d4',
        white: '#e6e6e6',
      },
      scrollback: 10000,
      cursorBlink: true,
      allowProposedApi: true,
    });

    fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.loadAddon(new WebLinksAddon());

    if (terminalEl) {
      terminal.open(terminalEl);
      // Canvas renderer (default) — WebGL addon removed to prevent GPU
      // resource leaks that degrade the X11 compositor over 24+ hours.
      fitAddon.fit();
    }

    // Create PTY session
    try {
      const info = await invoke<{ id: string }>('terminal_create', {
        cols: terminal.cols,
        rows: terminal.rows,
      });
      sessionId = info.id;
    } catch (e) {
      terminal.writeln(`\r\n\x1b[31mFailed to create terminal: ${e}\x1b[0m`);
      alive = false;
      return;
    }

    // PTY output -> xterm
    unlistenOutput = await listen<{ session_id: string; data: number[] }>('terminal-output', (event) => {
      if (event.payload.session_id === sessionId && terminal) {
        terminal.write(new Uint8Array(event.payload.data));
      }
    });

    // PTY exit
    unlistenExit = await listen<{ session_id: string }>('terminal-exit', (event) => {
      if (event.payload.session_id === sessionId) {
        alive = false;
        terminal?.writeln('\r\n\x1b[90m[Process exited]\x1b[0m');
      }
    });

    // User input -> PTY
    terminal.onData((data: string) => {
      if (sessionId && alive) {
        const bytes = Array.from(new TextEncoder().encode(data));
        invoke('terminal_write', { sessionId, data: bytes }).catch(() => {});
      }
    });

    // Terminal resize -> PTY resize
    terminal.onResize(({ cols, rows }) => {
      if (sessionId && alive) {
        invoke('terminal_resize', { sessionId, cols, rows }).catch(() => {});
      }
    });

    // Container resize observer
    if (terminalEl) {
      resizeObserver = new ResizeObserver(() => {
        requestAnimationFrame(() => fitAddon?.fit());
      });
      resizeObserver.observe(terminalEl);
    }
  }

  async function cleanup() {
    resizeObserver?.disconnect();
    unlistenOutput?.();
    unlistenExit?.();
    if (sessionId) {
      try { await invoke('terminal_destroy', { sessionId }); } catch {}
    }
    terminal?.dispose();
  }

  function handleClose() {
    cleanup();
    onclose();
  }
</script>

<div
  class="terminal-floating"
  data-zone="terminal"
  style="left:{x}px; top:{y}px; width:{width}px; height:{height}px;"
>
  <!-- Resize handles (8 edges/corners) -->
  <div class="resize-handle n" role="separator" onpointerdown={(e) => onResizePointerDown(e, 'n')} onpointermove={onResizePointerMove} onpointerup={onResizePointerUp}></div>
  <div class="resize-handle s" role="separator" onpointerdown={(e) => onResizePointerDown(e, 's')} onpointermove={onResizePointerMove} onpointerup={onResizePointerUp}></div>
  <div class="resize-handle e" role="separator" onpointerdown={(e) => onResizePointerDown(e, 'e')} onpointermove={onResizePointerMove} onpointerup={onResizePointerUp}></div>
  <div class="resize-handle w" role="separator" onpointerdown={(e) => onResizePointerDown(e, 'w')} onpointermove={onResizePointerMove} onpointerup={onResizePointerUp}></div>
  <div class="resize-handle ne" role="separator" onpointerdown={(e) => onResizePointerDown(e, 'ne')} onpointermove={onResizePointerMove} onpointerup={onResizePointerUp}></div>
  <div class="resize-handle nw" role="separator" onpointerdown={(e) => onResizePointerDown(e, 'nw')} onpointermove={onResizePointerMove} onpointerup={onResizePointerUp}></div>
  <div class="resize-handle se" role="separator" onpointerdown={(e) => onResizePointerDown(e, 'se')} onpointermove={onResizePointerMove} onpointerup={onResizePointerUp}></div>
  <div class="resize-handle sw" role="separator" onpointerdown={(e) => onResizePointerDown(e, 'sw')} onpointermove={onResizePointerMove} onpointerup={onResizePointerUp}></div>

  <!-- Header (drag handle) -->
  <div
    class="terminal-header"
    role="toolbar"
    tabindex="0"
    onpointerdown={onHeaderPointerDown}
    onpointermove={onHeaderPointerMove}
    onpointerup={onHeaderPointerUp}
  >
    <div class="header-left">
      <span class="terminal-icon">&#x1F4DF;</span>
      <span class="terminal-title">Terminal</span>
      {#if !alive}
        <span class="status-badge dead">exited</span>
      {/if}
    </div>
    <div class="header-actions">
      <button class="header-btn" onclick={onminimize} title="Minimize">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M5 12h14" />
        </svg>
      </button>
      <button class="header-btn close-btn" onclick={handleClose} title="Close">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M18 6L6 18M6 6l12 12" />
        </svg>
      </button>
    </div>
  </div>

  <!-- Terminal Container -->
  <div class="terminal-body" bind:this={terminalEl}></div>
</div>

<style>
  .terminal-floating {
    position: fixed;
    display: flex;
    flex-direction: column;
    z-index: 260;
    background: #0a0e14;
    border-radius: 8px;
    overflow: hidden;
    border: 1px solid rgba(6, 182, 212, 0.2);
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5), 0 0 24px rgba(6, 182, 212, 0.05);
  }

  .terminal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 12px;
    background: rgba(20, 25, 35, 0.95);
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
    user-select: none;
    flex-shrink: 0;
    cursor: grab;
  }

  .terminal-header:active {
    cursor: grabbing;
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .terminal-icon {
    font-size: 1rem;
  }

  .terminal-title {
    font-weight: 600;
    color: #e6e6e6;
    font-size: 0.929rem;
    font-family: system-ui, sans-serif;
  }

  .status-badge {
    font-size: 0.714rem;
    padding: 1px 6px;
    border-radius: 8px;
    font-weight: 600;
  }

  .status-badge.dead {
    background: rgba(239, 68, 68, 0.2);
    color: #ef4444;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .header-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: none;
    background: transparent;
    color: #888;
    border-radius: 4px;
    cursor: pointer;
    transition: background 0.15s, color 0.15s;
  }

  .header-btn:hover {
    background: rgba(255, 255, 255, 0.06);
    color: #e6e6e6;
  }

  .close-btn:hover {
    background: rgba(239, 68, 68, 0.2);
    color: #ef4444;
  }

  .terminal-body {
    flex: 1;
    padding: 4px;
    overflow: hidden;
  }

  .terminal-body :global(.xterm) {
    height: 100%;
  }

  .terminal-body :global(.xterm-viewport) {
    overflow-y: auto !important;
  }

  /* Resize handles */
  .resize-handle {
    position: absolute;
    z-index: 10;
  }

  .resize-handle.n { top: 0; left: 8px; right: 8px; height: 4px; cursor: n-resize; }
  .resize-handle.s { bottom: 0; left: 8px; right: 8px; height: 4px; cursor: s-resize; }
  .resize-handle.e { top: 8px; right: 0; bottom: 8px; width: 4px; cursor: e-resize; }
  .resize-handle.w { top: 8px; left: 0; bottom: 8px; width: 4px; cursor: w-resize; }
  .resize-handle.ne { top: 0; right: 0; width: 8px; height: 8px; cursor: ne-resize; }
  .resize-handle.nw { top: 0; left: 0; width: 8px; height: 8px; cursor: nw-resize; }
  .resize-handle.se { bottom: 0; right: 0; width: 8px; height: 8px; cursor: se-resize; }
  .resize-handle.sw { bottom: 0; left: 0; width: 8px; height: 8px; cursor: sw-resize; }
</style>
