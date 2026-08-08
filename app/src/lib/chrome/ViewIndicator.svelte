<script lang="ts">
  import { isGraphView } from '$lib/stores/canvas.svelte';

  let graphView = $derived(isGraphView());
</script>

<div class="view-indicator">
  <div class="dot" class:graph={graphView}></div>
  <span>{graphView ? 'Graph View' : 'Desk View'}</span>
</div>

<style>
  .view-indicator {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--card-bg);
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    padding: 6px 14px;
    font-size: 0.857rem;
    font-weight: 500;
    color: var(--text-secondary);
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--agent-accent);
    transition: background 0.3s;
  }

  .dot.graph {
    background: var(--infra-accent);
    animation: pulse 2s ease-in-out infinite;
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.5; }
  }
</style>
