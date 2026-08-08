<script lang="ts">
  interface Props {
    checked?: boolean;
    label?: string;
    disabled?: boolean;
  }

  let { checked = $bindable(false), label, disabled = false }: Props = $props();
</script>

<label class="toggle-group" class:disabled>
  <button
    role="switch"
    aria-checked={checked}
    aria-label={label ?? 'Toggle'}
    class="toggle-track"
    class:on={checked}
    {disabled}
    onclick={() => { if (!disabled) checked = !checked; }}
  >
    <span class="toggle-thumb"></span>
  </button>
  {#if label}
    <span class="toggle-label">{label}</span>
  {/if}
</label>

<style>
  .toggle-group {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    cursor: pointer;
  }

  .toggle-group.disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .toggle-track {
    position: relative;
    width: 36px;
    height: 20px;
    border-radius: 10px;
    background: var(--surface-border);
    border: none;
    cursor: inherit;
    padding: 0;
    transition: background var(--transition-speed, 200ms) ease;
    flex-shrink: 0;
  }

  .toggle-track.on {
    background: var(--infra-accent);
  }

  .toggle-thumb {
    position: absolute;
    left: 2px;
    top: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: var(--text-muted);
    transition: transform var(--transition-speed, 200ms) ease, background var(--transition-speed, 200ms) ease;
    box-shadow: 0 1px 3px rgba(0,0,0,0.3);
  }

  .toggle-track.on .toggle-thumb {
    transform: translateX(16px);
    background: #fff;
  }

  .toggle-label {
    font-size: 1rem;
    color: var(--text-primary);
    font-weight: var(--body-weight, 400);
  }

  .toggle-group.disabled .toggle-label {
    color: var(--text-muted);
  }
</style>
