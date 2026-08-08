<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    title: string;
    width?: string;
    onclose: () => void;
    children: Snippet;
  }

  let { title, width = '580px', onclose, children }: Props = $props();

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') onclose();
  }

  function handleOverlayKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onclose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div
  class="modal-overlay"
  role="button"
  tabindex="0"
  aria-label="Close modal"
  onclick={onclose}
  onkeydown={handleOverlayKeydown}
>
  <div
    class="modal-card"
    role="dialog"
    tabindex="-1"
    aria-modal="true"
    aria-labelledby="modal-shell-title"
    style="width: {width};"
    onclick={(e) => e.stopPropagation()}
    onkeydown={(e) => e.stopPropagation()}
  >
    <div class="modal-header">
      <h2 class="modal-title" id="modal-shell-title">{title}</h2>
      <button class="close-btn" onclick={onclose}>&times;</button>
    </div>
    <div class="modal-body">
      {@render children()}
    </div>
  </div>
</div>

<style>
  .modal-overlay {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .modal-card {
    background: var(--card-bg);
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    max-width: 95vw;
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 16px 20px;
    border-bottom: var(--border-width) solid var(--surface-border);
    flex-shrink: 0;
  }

  .modal-title {
    font-size: 1.143rem;
    font-weight: 700;
    color: var(--text-primary);
    margin: 0;
  }

  .close-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: 1.286rem;
    cursor: pointer;
    padding: 0 4px;
  }

  .close-btn:hover {
    color: var(--text-primary);
  }

  .modal-body {
    flex: 1;
    overflow-y: auto;
    padding: 0;
  }
</style>
