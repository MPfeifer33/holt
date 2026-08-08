<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import type { UnlistenFn } from '@tauri-apps/api/event';

  // ---------------------------------------------------------------------------
  // Types
  // ---------------------------------------------------------------------------

  interface A2ARelayMessage {
    from_id: string;
    from_name: string;
    to_id: string;
    to_name: string;
    content: string;
    timestamp: string;
  }

  // ---------------------------------------------------------------------------
  // Props
  // ---------------------------------------------------------------------------

  let {
    onclose,
    onminimize,
  }: {
    onclose: () => void;
    onminimize: () => void;
  } = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  const MAX_MESSAGES = 200;
  let messages = $state<A2ARelayMessage[]>([]);
  let scrollContainer: HTMLDivElement | undefined = $state();
  let autoScroll = $state(true);
  let unlistenRelay: UnlistenFn | undefined;

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------

  onMount(() => {
    const setup = async () => {
      unlistenRelay = await listen<A2ARelayMessage>('a2a-relay', (event) => {
        messages = [...messages.slice(-(MAX_MESSAGES - 1)), event.payload];
        if (autoScroll) {
          requestAnimationFrame(scrollToBottom);
        }
      });
    };
    setup();

    return () => {
      unlistenRelay?.();
    };
  });

  // ---------------------------------------------------------------------------
  // Scroll Management
  // ---------------------------------------------------------------------------

  function scrollToBottom() {
    if (scrollContainer) {
      scrollContainer.scrollTop = scrollContainer.scrollHeight;
    }
  }

  function handleScroll() {
    if (!scrollContainer) return;
    const { scrollTop, scrollHeight, clientHeight } = scrollContainer;
    // Auto-scroll if within 40px of bottom
    autoScroll = scrollHeight - scrollTop - clientHeight < 40;
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  function formatTime(isoTimestamp: string): string {
    try {
      const date = new Date(isoTimestamp);
      return date.toLocaleTimeString('en-US', { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' });
    } catch {
      return '??:??:??';
    }
  }

  function clearMessages() {
    messages = [];
  }
</script>

<div class="a2a-panel">
  <!-- Header -->
  <div class="panel-header">
    <div class="header-left">
      <span class="panel-icon">&#x1F4AC;</span>
      <span class="panel-title">Agent Comms</span>
      {#if messages.length > 0}
        <span class="message-count">{messages.length}</span>
      {/if}
    </div>
    <div class="header-actions">
      {#if messages.length > 0}
        <button class="header-btn" onclick={clearMessages} title="Clear messages">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M3 6h18M8 6V4a2 2 0 012-2h4a2 2 0 012 2v2m3 0v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6h14" />
          </svg>
        </button>
      {/if}
      <button class="header-btn" onclick={onminimize} title="Minimize">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M5 12h14" />
        </svg>
      </button>
      <button class="header-btn close-btn" onclick={onclose} title="Close">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M18 6L6 18M6 6l12 12" />
        </svg>
      </button>
    </div>
  </div>

  <!-- Message Stream -->
  <div
    class="message-stream"
    bind:this={scrollContainer}
    onscroll={handleScroll}
  >
    {#if messages.length === 0}
      <div class="empty-state">
        <p>No inter-agent messages yet.</p>
        <p class="empty-hint">Messages will appear here when agents communicate with each other.</p>
      </div>
    {:else}
      {#each messages as msg (msg.timestamp + msg.from_id + msg.to_id)}
        <div class="message-entry">
          <span class="msg-time">{formatTime(msg.timestamp)}</span>
          <span class="msg-from">{msg.from_name}</span>
          <span class="msg-arrow">&rarr;</span>
          <span class="msg-to">{msg.to_name}</span>
          <div class="msg-content">{msg.content}</div>
        </div>
      {/each}
    {/if}
  </div>

  <!-- Scroll indicator -->
  {#if !autoScroll && messages.length > 0}
    <button class="scroll-to-bottom" onclick={scrollToBottom}>
      New messages below &darr;
    </button>
  {/if}
</div>

<style>
  .a2a-panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--surface-0, rgba(10, 14, 20, 0.95));
    border-radius: 8px;
    overflow: hidden;
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
  }

  .panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    background: var(--surface-1, rgba(20, 25, 35, 0.9));
    border-bottom: 1px solid var(--border, rgba(255, 255, 255, 0.06));
    user-select: none;
    flex-shrink: 0;
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .panel-icon {
    font-size: 1rem;
  }

  .panel-title {
    font-weight: 600;
    color: var(--text-primary, #e6e6e6);
    font-size: 0.929rem;
  }

  .message-count {
    background: var(--accent, #06b6d4);
    color: var(--surface-0, #0a0e14);
    font-size: 0.714rem;
    padding: 1px 5px;
    border-radius: 8px;
    font-weight: 700;
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
    color: var(--text-muted, #888);
    border-radius: 4px;
    cursor: pointer;
    transition: background 0.15s, color 0.15s;
  }

  .header-btn:hover {
    background: var(--surface-2, rgba(255, 255, 255, 0.06));
    color: var(--text-primary, #e6e6e6);
  }

  .close-btn:hover {
    background: rgba(239, 68, 68, 0.2);
    color: #ef4444;
  }

  .message-stream {
    flex: 1;
    overflow-y: auto;
    padding: 8px 12px;
    scroll-behavior: smooth;
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted, #888);
    text-align: center;
    padding: 24px;
  }

  .empty-hint {
    font-size: 0.786rem;
    opacity: 0.6;
    margin-top: 4px;
  }

  .message-entry {
    padding: 6px 0;
    border-bottom: 1px solid var(--border, rgba(255, 255, 255, 0.03));
  }

  .message-entry:last-child {
    border-bottom: none;
  }

  .msg-time {
    color: var(--text-muted, #666);
    margin-right: 8px;
  }

  .msg-from {
    color: var(--accent, #06b6d4);
    font-weight: 600;
  }

  .msg-arrow {
    color: var(--text-muted, #666);
    margin: 0 4px;
  }

  .msg-to {
    color: #a78bfa;
    font-weight: 600;
  }

  .msg-content {
    margin-top: 4px;
    padding-left: 0;
    color: var(--text-primary, #e6e6e6);
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.4;
  }

  .scroll-to-bottom {
    position: absolute;
    bottom: 12px;
    left: 50%;
    transform: translateX(-50%);
    background: var(--accent, #06b6d4);
    color: var(--surface-0, #0a0e14);
    border: none;
    border-radius: 12px;
    padding: 4px 12px;
    font-size: 0.786rem;
    font-weight: 600;
    cursor: pointer;
    z-index: 10;
  }

  .scroll-to-bottom:hover {
    opacity: 0.9;
  }
</style>
