<script lang="ts">
  interface Props {
    label?: string;
    value?: string;
    placeholder?: string;
    error?: string;
    type?: string;
    disabled?: boolean;
  }

  let { label, value = $bindable(''), placeholder, error, type = 'text', disabled = false }: Props = $props();
</script>

<div class="input-group" class:has-error={!!error}>
  {#if label}
    <span class="input-label">{label}</span>
  {/if}
  <input
    class="input-field"
    {type}
    bind:value
    {placeholder}
    {disabled}
  />
  {#if error}
    <span class="input-error">{error}</span>
  {/if}
</div>

<style>
  .input-group {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .input-label {
    font-size: 0.714rem;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-weight: var(--body-weight, 400);
  }

  .input-field {
    background: var(--card-bg);
    border: var(--border-width, 1px) var(--border-style, solid) var(--surface-border);
    border-radius: var(--border-radius, 6px);
    padding: 8px 12px;
    font-size: 1rem;
    font-family: var(--font-family);
    font-weight: var(--body-weight, 400);
    color: var(--text-primary);
    transition: border-color var(--transition-speed, 200ms) ease, box-shadow var(--transition-speed, 200ms) ease;
    outline: none;
    width: 100%;
  }

  .input-field::placeholder {
    color: var(--text-muted);
  }

  .input-field:focus {
    border-color: var(--infra-accent);
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--infra-accent) 30%, transparent);
  }

  .input-field:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .has-error .input-field {
    border-color: var(--alert-accent);
  }

  .has-error .input-field:focus {
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--alert-accent) 30%, transparent);
  }

  .input-error {
    font-size: 0.714rem;
    color: var(--alert-accent);
  }
</style>
