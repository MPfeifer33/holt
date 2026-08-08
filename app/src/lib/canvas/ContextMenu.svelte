<script lang="ts">
  import { onMount, untrack } from 'svelte';

  export interface MenuItem {
    label: string;
    action: () => void;
    danger?: boolean;
    separator?: boolean;
  }

  let {
    items,
    x,
    y,
    onclose,
  }: {
    items: MenuItem[];
    x: number;
    y: number;
    onclose: () => void;
  } = $props();

  let menuEl: HTMLDivElement;

  // Snapshot props at component creation time (position is fixed per open).
  // untrack avoids the Svelte linter "captures initial value" warning while
  // still reading the prop exactly once at creation.
  const initialX = untrack(() => x);
  const initialY = untrack(() => y);

  // Clamped position — computed after mount when we know element dimensions
  let clampedX = $state(initialX);
  let clampedY = $state(initialY);

  onMount(() => {
    // Clamp to viewport edges
    const rect = menuEl.getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    clampedX = Math.min(initialX, vw - rect.width - 8);
    clampedY = Math.min(initialY, vh - rect.height - 8);

    // Focus for keyboard handling
    menuEl.focus();
  });

  function handleItemClick(item: MenuItem) {
    if (item.separator) return;
    item.action();
    onclose();
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      onclose();
    }
  }

  function handleBackdropClick(e: MouseEvent) {
    // Close when clicking outside the menu
    if (menuEl && !menuEl.contains(e.target as Node)) {
      onclose();
    }
  }
</script>

<!-- Invisible full-screen backdrop to catch outside clicks -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="context-backdrop" onclick={handleBackdropClick}></div>

<div
  class="context-menu"
  bind:this={menuEl}
  style="left: {clampedX}px; top: {clampedY}px;"
  role="menu"
  tabindex="-1"
  onkeydown={handleKeyDown}
>
  {#each items as item}
    {#if item.separator}
      <div class="menu-separator" role="separator"></div>
    {:else}
      <button
        class="menu-item"
        class:danger={item.danger}
        role="menuitem"
        onclick={() => handleItemClick(item)}
      >
        {item.label}
      </button>
    {/if}
  {/each}
</div>

<style>
  .context-backdrop {
    position: fixed;
    inset: 0;
    z-index: 49;
  }

  .context-menu {
    position: fixed;
    z-index: 50;
    background: var(--card-bg);
    border: var(--border-width, 1px) solid var(--surface-border);
    border-radius: var(--border-radius, 6px);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4), 0 2px 8px rgba(0, 0, 0, 0.2);
    padding: 4px 0;
    min-width: 180px;
    outline: none;
    overflow: hidden;
  }

  .menu-item {
    display: block;
    width: 100%;
    padding: 7px 14px;
    background: none;
    border: none;
    text-align: left;
    font-size: 0.929rem;
    color: var(--text-primary);
    cursor: pointer;
    white-space: nowrap;
    transition: background 0.1s ease;
  }

  .menu-item:hover {
    background: color-mix(in srgb, var(--surface-border) 40%, transparent);
  }

  .menu-item:active {
    background: color-mix(in srgb, var(--surface-border) 70%, transparent);
  }

  .menu-item.danger {
    color: var(--alert-accent);
  }

  .menu-item.danger:hover {
    background: color-mix(in srgb, var(--alert-accent) 12%, transparent);
  }

  .menu-separator {
    height: 1px;
    margin: 4px 0;
    background: var(--surface-border);
    opacity: 0.6;
  }
</style>
