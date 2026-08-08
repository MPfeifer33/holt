<script lang="ts">
  interface Props {
    label?: string;
    value?: string;
    options: Array<{ value: string; label: string }>;
    disabled?: boolean;
  }

  let { label, value = $bindable(''), options, disabled = false }: Props = $props();
</script>

<div class="select-group">
  {#if label}
    <span class="select-label">{label}</span>
  {/if}
  <div class="select-wrapper">
    <select class="select-field" bind:value {disabled}>
      {#each options as opt}
        <option value={opt.value}>{opt.label}</option>
      {/each}
    </select>
    <span class="select-arrow">▾</span>
  </div>
</div>

<style>
  .select-group {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .select-label {
    font-size: 0.714rem;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-weight: var(--body-weight, 400);
  }

  .select-wrapper {
    position: relative;
  }

  .select-field {
    appearance: none;
    background: var(--card-bg);
    border: var(--border-width, 1px) var(--border-style, solid) var(--surface-border);
    border-radius: var(--border-radius, 6px);
    padding: 8px 32px 8px 12px;
    font-size: 1rem;
    font-family: var(--font-family);
    font-weight: var(--body-weight, 400);
    color: var(--text-primary);
    width: 100%;
    cursor: pointer;
    outline: none;
    transition: border-color var(--transition-speed, 200ms) ease;
  }

  .select-field:focus {
    border-color: var(--infra-accent);
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--infra-accent) 30%, transparent);
  }

  .select-field:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .select-arrow {
    position: absolute;
    right: 10px;
    top: 50%;
    transform: translateY(-50%);
    color: var(--text-muted);
    pointer-events: none;
    font-size: 0.857rem;
  }
</style>
