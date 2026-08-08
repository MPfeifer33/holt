<script lang="ts">
  import { getCamera, zoomBy, resetCamera } from '$lib/stores/canvas.svelte';

  let cam = $derived(getCamera());
  let zoomPercent = $derived(Math.round(cam.zoom * 100));
</script>

<div class="zoom-controls">
  <button onclick={() => zoomBy(0.1)} title="Zoom in">+</button>
  <div class="zoom-value">{zoomPercent}%</div>
  <button onclick={() => zoomBy(-0.1)} title="Zoom out">−</button>
</div>

<style>
  .zoom-controls {
    display: flex;
    align-items: center;
    background: var(--card-bg);
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    padding: 2px;
    gap: 2px;
  }

  button {
    width: 32px;
    height: 32px;
    border: none;
    background: transparent;
    color: var(--text-secondary);
    border-radius: calc(var(--border-radius) - 2px);
    cursor: pointer;
    font-size: 1.143rem;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background 0.15s, color 0.15s;
  }

  button:hover {
    background: var(--surface-border);
    color: var(--text-primary);
  }

  .zoom-value {
    width: 48px;
    text-align: center;
    font-size: 0.786rem;
    font-family: var(--mono-family);
    color: var(--text-muted);
    border-left: 1px solid var(--surface-border);
    border-right: 1px solid var(--surface-border);
    padding: 0 4px;
  }
</style>
