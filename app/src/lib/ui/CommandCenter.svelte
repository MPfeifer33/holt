<script lang="ts">
  import type { Snippet } from 'svelte';
  import TabBar from './TabBar.svelte';
  import { fly, fade } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';

  interface Tab {
    id: string;
    label: string;
  }

  interface Props {
    tabs: Tab[];
    activeTab?: string;
    onclose: () => void;
    children: Snippet<[string]>;
  }

  let { tabs, activeTab = $bindable(tabs[0]?.id ?? ''), onclose, children }: Props = $props();

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') onclose();
  }

  function handleEdgeClick(e: MouseEvent) {
    if (e.target === e.currentTarget) onclose();
  }

  function handleOverlayKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' || e.key === ' ' || e.key === 'Escape') {
      e.preventDefault();
      onclose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div
  class="cc-overlay"
  role="button"
  tabindex="0"
  aria-label="Close command center"
  onclick={handleEdgeClick}
  onkeydown={handleOverlayKeydown}
  transition:fade={{ duration: 150 }}
>
  <div class="cc-modal" role="dialog" aria-modal="true" aria-label="Command Center" in:fly={{ y: -40, duration: 300, easing: cubicOut }} out:fade={{ duration: 150 }}>
    <div class="cc-header">
      <div class="cc-tabs">
        <TabBar {tabs} bind:activeTab />
      </div>
      <button class="cc-close" onclick={onclose} title="Close">&times;</button>
    </div>

    <div class="cc-content">
      {@render children(activeTab)}
    </div>
  </div>
</div>

<style>
  .cc-overlay {
    position: fixed;
    inset: 0;
    z-index: 300;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.3);
  }

  .cc-modal {
    position: absolute;
    top: 2.5%;
    left: 2.5%;
    right: 2.5%;
    bottom: 2.5%;
    background: color-mix(in srgb, var(--panel-bg) calc(var(--surface-opacity, 1) * 100%), transparent);
    backdrop-filter: blur(var(--backdrop-blur, 0px));
    border: var(--border-width, 1px) var(--border-style, solid)
      color-mix(in srgb, var(--surface-border) calc(var(--border-opacity, 1) * 100%), transparent);
    border-radius: calc(var(--border-radius, 8px) + 4px);
    box-shadow: var(--card-shadow, 0 25px 80px rgba(0,0,0,0.6));
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .cc-header {
    display: flex;
    align-items: stretch;
    justify-content: space-between;
    border-bottom: 1px solid var(--surface-border);
    flex-shrink: 0;
    padding: 0 16px;
  }

  .cc-tabs {
    flex: 1;
    min-width: 0;
  }

  .cc-tabs :global(.tab-bar) {
    border-bottom: none;
  }

  .cc-close {
    display: flex;
    align-items: center;
    padding: 0 8px;
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 1.143rem;
    transition: color var(--transition-speed, 200ms) ease;
  }

  .cc-close:hover {
    color: var(--text-primary);
  }

  .cc-content {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
</style>
