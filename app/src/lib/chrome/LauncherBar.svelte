<script lang="ts">
  type ModalId = 'connections' | 'observe' | 'plugins' | 'schedules' | 'skills' | 'theme' | 'settings';

  interface LauncherItem {
    id: ModalId;
    icon: string;
    label: string;
  }

  const ITEMS: LauncherItem[] = [
    { id: 'connections', icon: 'CN', label: 'Connections' },
    { id: 'observe', icon: 'OB', label: 'Observe' },
    { id: 'plugins', icon: 'PL', label: 'Plugins' },
    { id: 'schedules', icon: 'CR', label: 'Schedules' },
    { id: 'skills', icon: 'SK', label: 'Skills' },
    { id: 'theme', icon: 'TH', label: 'Theme' },
    { id: 'settings', icon: 'ST', label: 'Settings' },
  ];

  interface Props {
    onselect: (id: ModalId) => void;
    onclose: () => void;
  }

  let { onselect, onclose }: Props = $props();

  function handleBackdropKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' || e.key === ' ' || e.key === 'Escape') {
      e.preventDefault();
      onclose();
    }
  }

  export type { ModalId };
</script>

<div
  class="launcher-backdrop"
  role="button"
  tabindex="0"
  aria-label="Close launcher"
  onclick={onclose}
  onkeydown={handleBackdropKeydown}
></div>

<div class="launcher-bar">
  {#each ITEMS as item (item.id)}
    <button
      class="launcher-btn"
      onclick={() => onselect(item.id)}
      title={item.label}
    >
      <span class="launcher-icon">{item.icon}</span>
      <span class="launcher-label">{item.label}</span>
    </button>
  {/each}
</div>

<style>
  .launcher-backdrop {
    position: fixed;
    inset: 0;
    z-index: 59;
  }

  .launcher-bar {
    position: fixed;
    top: 50%;
    right: 0;
    transform: translateY(-50%);
    z-index: 60;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 0;
    background: var(--card-bg);
    border: var(--border-width) solid var(--surface-border);
    border-right: none;
    border-radius: var(--border-radius) 0 0 var(--border-radius);
    box-shadow: -4px 0 16px rgba(0, 0, 0, 0.3);
  }

  .launcher-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 14px;
    background: none;
    border: none;
    color: var(--text-secondary);
    cursor: pointer;
    white-space: nowrap;
    font-family: var(--font-family);
    font-size: 0.857rem;
    transition: background 0.1s, color 0.1s;
  }

  .launcher-btn:hover {
    background: color-mix(in srgb, var(--surface-border) 40%, transparent);
    color: var(--text-primary);
  }

  .launcher-icon {
    font-family: var(--mono-family);
    font-size: 0.786rem;
    font-weight: 700;
    color: var(--text-muted);
    width: 22px;
    text-align: center;
    flex-shrink: 0;
  }

  .launcher-label {
    font-weight: 500;
  }
</style>
