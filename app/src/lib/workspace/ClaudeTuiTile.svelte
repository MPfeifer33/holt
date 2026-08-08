<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { Terminal, type IDisposable } from 'xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { WebLinksAddon } from '@xterm/addon-web-links';
  import 'xterm/css/xterm.css';
  import ProjectSwitcher from './ProjectSwitcher.svelte';

  // ---------------------------------------------------------------------------
  // Props
  // ---------------------------------------------------------------------------

  interface Project {
    name: string;
    path: string;
    session_id?: string | null;
  }

  interface Props {
    agentId: string;
  }

  let { agentId }: Props = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  let terminalEl: HTMLDivElement | undefined = $state();
  let sessionId: string | null = $state(null);
  let alive = $state(false);
  let starting = $state(false);
  let terminal: Terminal | undefined;
  let fitAddon: FitAddon | undefined;
  let unlistenOutput: UnlistenFn | undefined;
  let unlistenExit: UnlistenFn | undefined;
  let resizeObserver: ResizeObserver | undefined;
  // xterm registers IDisposable handlers we have to dispose ourselves on
  // restart/switch, otherwise each cycle leaks an onData/onResize handler
  // and a single keystroke fires N invoke('terminal_write') calls.
  let dataDisposable: IDisposable | undefined;
  let resizeDisposable: IDisposable | undefined;
  let currentProject: Project | null = $state(null);

  // ---------------------------------------------------------------------------
  // Terminal Lifecycle
  // ---------------------------------------------------------------------------

  onMount(() => {
    initTerminal();
    return () => { cleanup(); };
  });

  function initTerminal() {
    terminal = new Terminal({
      fontFamily: '"JetBrains Mono", "Fira Code", "Cascadia Code", monospace',
      fontSize: 14,
      theme: {
        background: '#0a0e14',
        foreground: '#e6e6e6',
        cursor: '#f97316',
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
      scrollback: 50000,
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

    // Container resize observer
    if (terminalEl) {
      resizeObserver = new ResizeObserver(() => {
        requestAnimationFrame(() => fitAddon?.fit());
      });
      resizeObserver.observe(terminalEl);
    }

    // Auto-start Claude Code
    startClaude();
  }

  async function startClaude() {
    if (!terminal || starting || alive) return;
    starting = true;

    terminal.writeln('\x1b[38;2;249;115;22m\x1b[1m--- Claude Code TUI Lane ---\x1b[0m');
    terminal.writeln('\x1b[90mStarting Claude Code with Bridge + Holt MCP...\x1b[0m');
    terminal.writeln('');

    try {
      const info = await invoke<{ id: string }>('claude_tui_start', {
        agentId,
        projectPath: currentProject?.path ?? null,
        cols: terminal.cols,
        rows: terminal.rows,
      });
      sessionId = info.id;
      alive = true;

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
          terminal?.writeln('\r\n\x1b[90m[Claude Code exited — use Restart to relaunch]\x1b[0m');
        }
      });

      // User input -> PTY. Track the disposable so we can drop it on
      // stop/restart/switch — otherwise each cycle leaks a handler.
      dataDisposable = terminal.onData((data: string) => {
        if (sessionId && alive) {
          const bytes = Array.from(new TextEncoder().encode(data));
          invoke('terminal_write', { sessionId, data: bytes }).catch(() => {});
        }
      });

      // Terminal resize -> PTY resize. Same disposal contract as onData.
      resizeDisposable = terminal.onResize(({ cols, rows }) => {
        if (sessionId && alive) {
          invoke('terminal_resize', { sessionId, cols, rows }).catch(() => {});
        }
      });
    } catch (e) {
      terminal.writeln(`\r\n\x1b[31mFailed to start Claude Code: ${e}\x1b[0m`);
      terminal.writeln('\x1b[90mMake sure "claude" is in your PATH.\x1b[0m');
    } finally {
      starting = false;
    }
  }

  async function stopClaude() {
    if (sessionId) {
      unlistenOutput?.();
      unlistenExit?.();
      unlistenOutput = undefined;
      unlistenExit = undefined;
      dataDisposable?.dispose();
      resizeDisposable?.dispose();
      dataDisposable = undefined;
      resizeDisposable = undefined;
      try { await invoke('terminal_destroy', { sessionId }); } catch {}
      sessionId = null;
      alive = false;
      terminal?.writeln('\r\n\x1b[90m[Stopped]\x1b[0m');
    }
  }

  async function handleProjectSwitch(project: Project) {
    if (currentProject?.path === project.path && alive) return;
    currentProject = project;
    terminal?.writeln(`\r\n\x1b[90mSwitching to ${project.name}...\x1b[0m`);
    await stopClaude();
    await startClaude();
  }

  async function restartClaude() {
    await stopClaude();
    terminal?.writeln('\x1b[90mRestarting...\x1b[0m\r\n');
    await startClaude();
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
</script>

<div class="claude-tui-tile">
  <div class="tui-toolbar">
    <ProjectSwitcher
      {agentId}
      {currentProject}
      onSwitch={handleProjectSwitch}
    />
    <div class="tui-status">
      <span class="status-dot" class:alive class:starting></span>
      {#if starting}
        Starting...
      {:else if alive}
        Connected
      {:else}
        Stopped
      {/if}
    </div>
    <div class="tui-actions">
      {#if alive}
        <button class="tui-btn" onclick={stopClaude} title="Stop Claude Code">Stop</button>
      {:else if !starting}
        <button class="tui-btn primary" onclick={startClaude} title="Start Claude Code">Start</button>
      {/if}
      <button class="tui-btn" onclick={restartClaude} title="Restart Claude Code" disabled={starting}>Restart</button>
    </div>
  </div>
  <div class="terminal-container" bind:this={terminalEl}></div>
</div>

<style>
  .claude-tui-tile {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    overflow: hidden;
  }

  .tui-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 8px;
    background: rgba(10, 14, 20, 0.8);
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
    flex-shrink: 0;
  }

  .tui-status {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 0.75rem;
    color: var(--text-muted, #888);
  }

  .status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #666;
  }

  .status-dot.alive {
    background: #22c55e;
    box-shadow: 0 0 6px rgba(34, 197, 94, 0.5);
  }

  .status-dot.starting {
    background: #f59e0b;
    animation: pulse 1s ease-in-out infinite;
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.4; }
  }

  .tui-actions {
    display: flex;
    gap: 4px;
  }

  .tui-btn {
    padding: 2px 10px;
    font-size: 0.7rem;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.04);
    color: var(--text-muted, #888);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .tui-btn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text-primary, #e6e6e6);
  }

  .tui-btn.primary {
    border-color: rgba(249, 115, 22, 0.3);
    color: #f97316;
  }

  .tui-btn.primary:hover {
    background: rgba(249, 115, 22, 0.1);
  }

  .tui-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .terminal-container {
    flex: 1;
    overflow: hidden;
  }

  /* xterm.js container needs explicit dimensions */
  .terminal-container :global(.xterm) {
    height: 100%;
    padding: 4px;
  }
</style>
