<script lang="ts">
  import { hasAnyWindows } from '$lib/stores/windows.svelte';

  interface Props {
    mode: 'canvas' | 'chat';
    ontoggle: () => void;
  }

  let { mode, ontoggle }: Props = $props();

  let label = $derived(mode === 'canvas' ? 'Chat' : 'Canvas');
  let isDisabled = $derived(mode === 'canvas' && !hasAnyWindows());
</script>

<button
  class="mode-toggle"
  class:disabled={isDisabled}
  onclick={ontoggle}
  disabled={isDisabled}
>
  {label}
</button>

<style>
  .mode-toggle {
    position: fixed;
    top: 12px;
    left: 14px;
    z-index: 200;
    padding: 6px 16px;
    background: var(--card-bg);
    border: var(--border-width, 1px) var(--border-style, solid) var(--surface-border);
    border-radius: var(--border-radius, 8px);
    font-size: 1rem;
    font-family: var(--font-family);
    font-weight: var(--body-weight, 400);
    color: var(--text-primary);
    cursor: pointer;
    transition: all var(--transition-speed, 200ms) ease;
  }

  .mode-toggle.disabled {
    color: var(--text-muted);
    opacity: 0.5;
    cursor: not-allowed;
  }

  .mode-toggle:hover:not(.disabled) {
    background: color-mix(in srgb, var(--surface-border) 20%, var(--card-bg));
  }
</style>
