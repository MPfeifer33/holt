<script lang="ts">
  interface Props {
    label?: string;
    value?: string;
    placeholder?: string;
    monospace?: boolean;
    rows?: number;
    error?: string;
    disabled?: boolean;
  }

  let { label, value = $bindable(''), placeholder, monospace = false, rows = 3, error, disabled = false }: Props = $props();
</script>

<div class="textarea-group" class:has-error={!!error}>
  {#if label}
    <span class="textarea-label">{label}</span>
  {/if}
  <textarea
    class="textarea-field"
    class:monospace
    bind:value
    {placeholder}
    {rows}
    {disabled}
  ></textarea>
  {#if error}
    <span class="textarea-error">{error}</span>
  {/if}
</div>

<style>
  .textarea-group {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .textarea-label {
    font-size: 0.714rem;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-weight: var(--body-weight, 400);
  }

  .textarea-field {
    background: var(--card-bg);
    border: var(--border-width, 1px) var(--border-style, solid) var(--surface-border);
    border-radius: var(--border-radius, 6px);
    padding: 10px 12px;
    font-size: 1rem;
    font-family: var(--font-family);
    font-weight: var(--body-weight, 400);
    color: var(--text-primary);
    resize: vertical;
    outline: none;
    width: 100%;
    transition: border-color var(--transition-speed, 200ms) ease;
  }

  .textarea-field.monospace {
    font-family: var(--mono-family);
  }

  .textarea-field::placeholder {
    color: var(--text-muted);
  }

  .textarea-field:focus {
    border-color: var(--infra-accent);
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--infra-accent) 30%, transparent);
  }

  .textarea-field:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .has-error .textarea-field {
    border-color: var(--alert-accent);
  }

  .textarea-error {
    font-size: 0.714rem;
    color: var(--alert-accent);
  }
</style>
