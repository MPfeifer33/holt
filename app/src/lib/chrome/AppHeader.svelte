<script lang="ts">
  import { onMount } from 'svelte';
  import { getAppMetadata } from '$lib/tauri/commands';

  let appName = $state('Holt');
  let appVersion = $state('');

  onMount(async () => {
    try {
      const meta = await getAppMetadata();
      appName = meta.name;
      appVersion = `v${meta.version}`;
    } catch {
      appVersion = '';
    }
  });
</script>

<div class="app-header-pill">
  <div class="icon" aria-hidden="true"></div>
  <span class="brand">{appName}</span>
  {#if appVersion}
    <span class="version">{appVersion}</span>
  {/if}
</div>

<style>
  .app-header-pill {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    background: var(--card-bg);
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    padding: 7px 14px;
    pointer-events: auto;
  }

  .icon {
    width: 18px;
    height: 18px;
    border-radius: 5px;
    background: linear-gradient(135deg, #6366f1, #06b6d4);
    flex-shrink: 0;
  }

  .brand {
    font-weight: 700;
    font-size: 1.071rem;
    color: var(--text-primary);
    letter-spacing: 0.01em;
  }

  .version {
    font-size: 0.714rem;
    color: var(--text-muted);
    font-family: var(--mono-family);
    margin-top: 1px;
  }
</style>
