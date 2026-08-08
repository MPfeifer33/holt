<script lang="ts">
  import { onMount } from 'svelte';

  type ModalMode = 'prompt' | 'alert' | 'select';

  interface Props {
    title: string;
    message?: string;
    mode?: ModalMode;
    placeholder?: string;
    defaultValue?: string;
    /** For select mode: list of options to choose from */
    options?: { label: string; value: string }[];
    confirmLabel?: string;
    cancelLabel?: string;
    danger?: boolean;
    onconfirm: (value: string) => void;
    oncancel: () => void;
  }

  let {
    title,
    message = '',
    mode = 'prompt',
    placeholder = '',
    defaultValue = '',
    options = [],
    confirmLabel = 'OK',
    cancelLabel = 'Cancel',
    danger = false,
    onconfirm,
    oncancel,
  }: Props = $props();

  let inputValue = $state('');
  let selectedIndex = $state(-1);
  let inputEl: HTMLInputElement | undefined = $state();

  onMount(() => {
    inputValue = defaultValue;
    if (mode === 'prompt' && inputEl) {
      inputEl.focus();
      inputEl.select();
    }
  });

  function handleConfirm() {
    if (mode === 'select') {
      if (selectedIndex >= 0 && selectedIndex < options.length) {
        onconfirm(options[selectedIndex].value);
      }
    } else {
      onconfirm(inputValue);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      oncancel();
    } else if (e.key === 'Enter' && mode !== 'select') {
      e.preventDefault();
      handleConfirm();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="modal-backdrop" onclick={oncancel} onkeydown={undefined}>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal-card" onclick={(e) => e.stopPropagation()} onkeydown={undefined}>
    <div class="modal-header">
      <h3 class="modal-title">{title}</h3>
      <button class="close-btn" onclick={oncancel} aria-label="Close">&#10005;</button>
    </div>

    {#if message}
      <div class="modal-message">{message}</div>
    {/if}

    <div class="modal-body">
      {#if mode === 'prompt'}
        <input
          bind:this={inputEl}
          bind:value={inputValue}
          class="modal-input"
          type="text"
          {placeholder}
        />
      {:else if mode === 'select'}
        <div class="option-list">
          {#each options as opt, i}
            <button
              class="option-btn"
              class:selected={selectedIndex === i}
              onclick={() => { selectedIndex = i; }}
              ondblclick={() => { selectedIndex = i; handleConfirm(); }}
            >
              <span class="option-index">{i + 1}.</span>
              <span class="option-label">{opt.label}</span>
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <div class="modal-actions">
      {#if mode !== 'alert'}
        <button class="btn btn-cancel" onclick={oncancel}>{cancelLabel}</button>
      {/if}
      <button
        class="btn btn-confirm"
        class:btn-danger={danger}
        onclick={handleConfirm}
        disabled={mode === 'select' && selectedIndex < 0}
      >{confirmLabel}</button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    z-index: 10000;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    animation: fade-in 0.15s ease-out;
  }

  @keyframes fade-in {
    from { opacity: 0; }
    to { opacity: 1; }
  }

  .modal-card {
    width: 420px;
    max-width: calc(100vw - 32px);
    max-height: calc(100vh - 64px);
    background: var(--card-bg, rgba(15, 23, 42, 0.98));
    border: 1px solid var(--surface-border, #334155);
    border-radius: var(--border-radius, 8px);
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.6);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    animation: modal-enter 0.15s ease-out;
  }

  @keyframes modal-enter {
    from { opacity: 0; transform: scale(0.96) translateY(8px); }
    to { opacity: 1; transform: scale(1) translateY(0); }
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px 8px;
  }

  .modal-title {
    font-size: 1rem;
    font-weight: 700;
    color: var(--text-primary, #e2e8f0);
    margin: 0;
  }

  .close-btn {
    background: none;
    border: none;
    color: var(--text-muted, #64748b);
    font-size: 0.857rem;
    cursor: pointer;
    padding: 2px 4px;
    border-radius: 3px;
    line-height: 1;
  }

  .close-btn:hover {
    color: var(--text-primary, #e2e8f0);
    background: rgba(255, 255, 255, 0.08);
  }

  .modal-message {
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    color: var(--text-secondary, #cbd5e1);
    padding: 0 14px 8px;
    line-height: 1.5;
    white-space: pre-wrap;
  }

  .modal-body {
    padding: 0 14px 12px;
  }

  .modal-input {
    width: 100%;
    padding: 8px 10px;
    font-family: var(--mono-family, monospace);
    font-size: 0.929rem;
    color: var(--text-primary, #e2e8f0);
    background: rgba(0, 0, 0, 0.25);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 4px;
    outline: none;
    box-sizing: border-box;
  }

  .modal-input:focus {
    border-color: var(--agent-accent, #06b6d4);
  }

  .option-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 240px;
    overflow-y: auto;
  }

  .option-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    border: 1px solid transparent;
    border-radius: 4px;
    background: transparent;
    color: var(--text-secondary, #cbd5e1);
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    cursor: pointer;
    text-align: left;
    transition: background 100ms, border-color 100ms;
  }

  .option-btn:hover {
    background: rgba(255, 255, 255, 0.04);
  }

  .option-btn.selected {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 12%, transparent);
    border-color: var(--agent-accent, #06b6d4);
    color: var(--text-primary, #e2e8f0);
  }

  .option-index {
    color: var(--text-muted, #64748b);
    font-size: 0.786rem;
    min-width: 18px;
  }

  .option-label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
    padding: 0 14px 12px;
  }

  .btn {
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    font-weight: 600;
    padding: 6px 14px;
    border-radius: 4px;
    border: 1px solid var(--surface-border, #334155);
    cursor: pointer;
    transition: background 120ms, color 120ms;
  }

  .btn-cancel {
    background: transparent;
    color: var(--text-secondary, #cbd5e1);
  }

  .btn-cancel:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--text-primary, #e2e8f0);
  }

  .btn-confirm {
    background: var(--agent-accent, #06b6d4);
    border-color: var(--agent-accent, #06b6d4);
    color: #000;
  }

  .btn-confirm:hover {
    opacity: 0.9;
  }

  .btn-confirm:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .btn-danger {
    background: var(--status-error, #ef4444);
    border-color: var(--status-error, #ef4444);
  }
</style>
