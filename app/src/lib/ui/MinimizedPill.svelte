<script lang="ts">
  interface Props {
    agentId: string;
    name: string;
    statusColor: string;
    onclick: () => void;
  }

  let { agentId, name, statusColor, onclick }: Props = $props();
</script>

<button class="pill" {onclick} title="Restore {name}">
  <span class="pill-dot" style="background: {statusColor};"></span>
  <span class="pill-name">{name}</span>
</button>

<style>
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 12px;
    background: var(--card-bg);
    border: var(--border-width, 1px) var(--border-style, solid) var(--surface-border);
    border-radius: var(--border-radius, 6px);
    cursor: pointer;
    transition: background var(--transition-speed, 200ms) ease;
    font-family: var(--font-family);
  }

  .pill:hover {
    background: color-mix(in srgb, var(--surface-border) 25%, var(--card-bg));
  }

  .pill-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .pill-name {
    font-size: 0.857rem;
    color: var(--text-secondary);
    white-space: nowrap;
  }
</style>
