<script lang="ts">
  interface Props {
    onclick: () => void;
  }

  let { onclick }: Props = $props();
</script>

<button class="cc-button" {onclick}>
  Command Center
</button>

<style>
  .cc-button {
    position: fixed;
    top: 12px;
    right: 14px;
    z-index: 200;
    padding: 6px 16px;
    background: linear-gradient(135deg,
      color-mix(in srgb, var(--infra-accent) 15%, transparent),
      color-mix(in srgb, var(--agent-accent) 15%, transparent));
    border: var(--border-width, 1px) var(--border-style, solid)
      color-mix(in srgb, var(--infra-accent) 30%, transparent);
    border-radius: var(--border-radius, 8px);
    font-size: 1rem;
    font-family: var(--font-family);
    font-weight: var(--body-weight, 400);
    color: var(--text-primary);
    cursor: pointer;
    transition: all var(--transition-speed, 200ms) ease;
  }

  .cc-button:hover {
    background: linear-gradient(135deg,
      color-mix(in srgb, var(--infra-accent) 25%, transparent),
      color-mix(in srgb, var(--agent-accent) 25%, transparent));
  }
</style>
