<script lang="ts">
  interface Tab {
    id: string;
    label: string;
  }

  interface Props {
    tabs: Tab[];
    activeTab?: string;
    onchange?: (tabId: string) => void;
  }

  let { tabs, activeTab = $bindable(tabs[0]?.id ?? ''), onchange }: Props = $props();

  function selectTab(id: string) {
    activeTab = id;
    onchange?.(id);
  }
</script>

<div class="tab-bar">
  {#each tabs as tab}
    <button
      class="tab"
      class:active={tab.id === activeTab}
      onclick={() => selectTab(tab.id)}
    >
      {tab.label}
    </button>
  {/each}
</div>

<style>
  .tab-bar {
    display: flex;
    gap: 0;
    border-bottom: 1px solid var(--surface-border);
  }

  .tab {
    padding: 10px 18px;
    font-size: 1rem;
    font-family: var(--font-family);
    font-weight: var(--body-weight, 400);
    color: var(--text-muted);
    background: none;
    border: none;
    cursor: pointer;
    border-bottom: 2px solid transparent;
    transition: color var(--transition-speed, 200ms) ease, border-color var(--transition-speed, 200ms) ease;
    white-space: nowrap;
  }

  .tab:hover {
    color: var(--text-secondary);
  }

  .tab.active {
    color: var(--text-primary);
    font-weight: var(--heading-weight, 600);
    border-bottom-color: var(--infra-accent);
  }
</style>
