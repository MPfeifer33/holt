<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { listenAgentStream } from '$lib/tauri/events';
  import type { StreamEvent } from '$lib/tauri/events';
  import type { UnlistenFn } from '@tauri-apps/api/event';

  interface Props {
    agentId: string;
  }

  let { agentId }: Props = $props();

  interface BashEntry {
    id: number;
    toolCallId?: string;
    command: string;
    output: string | null;
    timestamp: number;
    status: 'done';
    durationMs?: number;
  }

  let entries = $state<BashEntry[]>([]);
  let nextId = 0;
  let scrollEl: HTMLDivElement | undefined = $state();
  let unlisteners: UnlistenFn[] = [];

  function scrollToBottom() {
    requestAnimationFrame(() => {
      if (scrollEl) scrollEl.scrollTop = scrollEl.scrollHeight;
    });
  }

  $effect(() => {
    void entries.length;
    scrollToBottom();
  });

  function extractCommand(args: unknown): string {
    if (typeof args === 'object' && args !== null) {
      const a = args as Record<string, unknown>;
      if (typeof a.command === 'string') return a.command;
      if (typeof a.cmd === 'string') return a.cmd;
    }
    if (typeof args === 'string') {
      try {
        const parsed = JSON.parse(args);
        return parsed.command ?? parsed.cmd ?? args;
      } catch {
        return args;
      }
    }
    return String(args ?? '');
  }

  function extractOutput(result: unknown): string | null {
    if (result === null || result === undefined) return null;
    if (typeof result === 'string') return result || null;
    if (typeof result === 'object') {
      const r = result as Record<string, unknown>;
      // Tool results are JSON objects — extract output/stdout or stringify
      if (typeof r.output === 'string') return r.output || null;
      if (typeof r.stdout === 'string') return r.stdout || null;
      if (typeof r.error === 'string') return r.error;
      return JSON.stringify(result, null, 2);
    }
    return String(result);
  }

  function setupListener() {
    listenAgentStream((event: StreamEvent) => {
      if (event.agent_id !== agentId) return;

      if (event.event_type === 'ToolCall') {
        const toolName = (event.data.tool as string) ?? '';
        const lower = toolName.toLowerCase();
        if (lower !== 'bash' && lower !== 'run_bash' && lower !== 'execute_bash') return;

        const toolCallId = typeof event.data.tool_call_id === 'string' ? event.data.tool_call_id : undefined;
        const command = extractCommand(event.data.arguments);
        const output = extractOutput(event.data.result);
        const durationMs = event.data.duration_ms as number | undefined;

        if (output) {
          entries = [...entries, {
            id: nextId++,
            toolCallId,
            command,
            output,
            timestamp: Date.now(),
            status: 'done',
            durationMs,
          }];
        } else {
          entries = [...entries, {
            id: nextId++,
            toolCallId,
            command,
            output: null,
            timestamp: Date.now(),
            status: 'done',
          }];
        }
      } else if (event.event_type === 'ToolResult') {
        const toolName = (event.data.tool as string) ?? '';
        const lower = toolName.toLowerCase();
        if (lower !== 'bash' && lower !== 'run_bash' && lower !== 'execute_bash') return;

        const toolCallId = typeof event.data.tool_call_id === 'string' ? event.data.tool_call_id : undefined;
        const output = extractOutput(event.data.result);
        const durationMs = event.data.duration_ms as number | undefined;
        const existing = toolCallId ? entries.findLast((entry) => entry.toolCallId === toolCallId) : undefined;
        if (existing) {
          entries = entries.map(e =>
            e === existing ? { ...e, output, durationMs } : e
          );
        } else {
          entries = [...entries, {
            id: nextId++,
            toolCallId,
            command: '',
            output,
            timestamp: Date.now(),
            status: 'done',
            durationMs,
          }];
        }
      }
    }).then(fn => unlisteners.push(fn));
  }

  onMount(() => {
    setupListener();
  });

  onDestroy(() => {
    for (const unlisten of unlisteners) unlisten();
    unlisteners = [];
  });

  function formatTime(ts: number): string {
    return new Date(ts).toLocaleTimeString(undefined, {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });
  }
</script>

<div class="terminal-tile" bind:this={scrollEl}>
  {#if entries.length === 0}
    <div class="empty-state">
      <span class="prompt">$</span>
      <span class="empty-text"> Waiting for bash commands…</span>
      <span class="cursor">_</span>
    </div>
  {:else}
    {#each entries as entry (entry.id)}
      <div class="bash-entry">
        <div class="entry-header">
          <span class="prompt">$</span>
          <span class="command">{entry.command}</span>
          <span class="entry-time">
            {formatTime(entry.timestamp)}{#if entry.durationMs} · {entry.durationMs < 1000 ? `${entry.durationMs}ms` : `${(entry.durationMs / 1000).toFixed(1)}s`}{/if}
          </span>
        </div>
        {#if entry.output}
          <pre class="entry-output">{entry.output}</pre>
        {/if}
      </div>
    {/each}
  {/if}
</div>

<style>
  .terminal-tile {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow-y: auto;
    background: #0a0e14;
    padding: 10px 12px;
    gap: 10px;
    font-family: var(--mono-family, 'Fira Code', 'Cascadia Code', monospace);
    font-size: 0.857rem;
    color: #c9d1d9;
  }

  .empty-state {
    display: flex;
    align-items: center;
    gap: 4px;
    color: #4a5568;
    padding: 4px 0;
  }

  .cursor {
    animation: blink 1s steps(1) infinite;
  }

  @keyframes blink {
    0%, 100% { opacity: 1; }
    50% { opacity: 0; }
  }

  .bash-entry {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .entry-header {
    display: flex;
    align-items: baseline;
    gap: 6px;
    flex-wrap: wrap;
  }

  .prompt {
    color: #58a6ff;
    font-weight: 700;
    flex-shrink: 0;
  }

  .command {
    color: #e6edf3;
    font-weight: 500;
    flex: 1;
    white-space: pre-wrap;
    word-break: break-all;
  }

  .entry-time {
    color: #4a5568;
    font-size: 0.714rem;
    flex-shrink: 0;
    margin-left: auto;
  }

  .entry-output {
    margin: 0;
    padding: 6px 10px;
    background: #0d1117;
    border-left: 2px solid #21262d;
    color: #8b949e;
    font-size: 0.786rem;
    font-family: inherit;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-all;
    max-height: 300px;
    overflow-y: auto;
    border-radius: 0 3px 3px 0;
  }

  .empty-text {
    color: #4a5568;
  }
</style>
