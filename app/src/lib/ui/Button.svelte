<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    variant?: 'primary' | 'secondary' | 'danger' | 'ghost' | 'icon';
    disabled?: boolean;
    onclick?: (e: MouseEvent) => void;
    children: Snippet;
  }

  let { variant = 'secondary', disabled = false, onclick, children }: Props = $props();
</script>

<button
  class="btn btn-{variant}"
  {disabled}
  {onclick}
>
  {@render children()}
</button>

<style>
  .btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 7px 16px;
    font-size: 1rem;
    font-family: var(--font-family);
    font-weight: var(--body-weight, 400);
    letter-spacing: var(--letter-spacing, 0);
    border-radius: var(--border-radius, 6px);
    border-width: var(--border-width, 1px);
    border-style: var(--border-style, solid);
    cursor: pointer;
    transition: all var(--transition-speed, 200ms) ease;
    line-height: 1.4;
    white-space: nowrap;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-primary {
    background: var(--infra-accent);
    color: #fff;
    border-color: var(--infra-accent);
  }
  .btn-primary:hover:not(:disabled) {
    filter: brightness(1.15);
  }

  .btn-secondary {
    background: transparent;
    color: var(--text-primary);
    border-color: var(--surface-border);
  }
  .btn-secondary:hover:not(:disabled) {
    background: color-mix(in srgb, var(--surface-border) 20%, transparent);
  }

  .btn-danger {
    background: color-mix(in srgb, var(--alert-accent) 15%, transparent);
    color: var(--alert-accent);
    border-color: color-mix(in srgb, var(--alert-accent) 30%, transparent);
  }
  .btn-danger:hover:not(:disabled) {
    background: color-mix(in srgb, var(--alert-accent) 25%, transparent);
  }

  .btn-ghost {
    background: transparent;
    color: var(--text-secondary);
    border: none;
    padding: 7px 12px;
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .btn-ghost:hover:not(:disabled) {
    color: var(--text-primary);
  }

  .btn-icon {
    background: var(--card-bg);
    color: var(--text-secondary);
    border-color: var(--surface-border);
    padding: 5px 8px;
    font-size: 0.857rem;
  }
  .btn-icon:hover:not(:disabled) {
    color: var(--text-primary);
    background: color-mix(in srgb, var(--surface-border) 25%, var(--card-bg));
  }

  .btn:focus-visible {
    outline: 2px solid var(--infra-accent);
    outline-offset: 2px;
  }
</style>
