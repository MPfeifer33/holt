<script lang="ts">
  import { onMount } from 'svelte';
  import {
    getAppSettings,
    getSearchConfig,
    updateSystemSettings,
    updateSearchConfig,
    updateTraceSettings,
    type WebToolsConfig,
  } from '$lib/tauri/commands';
  import { invoke } from '@tauri-apps/api/core';
  import {
    DEFAULT_RETENTION_DAYS,
    SEARCH_MAX_RESULTS,
  } from '$lib/constants';


  // =====================
  // App Settings
  // =====================

  let minimizeToTray = $state(true);

  async function loadAppSettings() {
    try {
      const settings = await getAppSettings();
      minimizeToTray = settings.minimize_to_tray;
    } catch {
      // fallback to default
    }
  }

  async function toggleMinimizeToTray() {
    minimizeToTray = !minimizeToTray;
    try {
      await updateSystemSettings({ minimizeToTray });
    } catch {
      minimizeToTray = !minimizeToTray; // revert on failure
    }
  }

  // =====================
  // Web Search Config
  // =====================

  let searchConfig = $state({
    provider: '',
    endpoint: '',
    maxResults: SEARCH_MAX_RESULTS,
  });
  let searchLoaded = $state(false);
  let searchSaving = $state(false);

  async function loadSearchConfig() {
    try {
      const config: WebToolsConfig = await getSearchConfig();
      searchConfig.provider = config.search_provider;
      searchConfig.endpoint = config.search_endpoint;
      searchLoaded = true;
    } catch {
      // backend may not have this command
      searchLoaded = true;
    }
  }

  async function saveSearchConfig() {
    searchSaving = true;
    try {
      await updateSearchConfig({
        provider: searchConfig.provider || undefined,
        endpoint: searchConfig.endpoint || undefined,
      });
    } catch {
      // silently ignore
    } finally {
      searchSaving = false;
    }
  }

  // =====================
  // Trace Settings
  // =====================

  let retentionDays = $state(DEFAULT_RETENTION_DAYS);
  let traceSaving = $state(false);

  async function saveTraceRetention() {
    traceSaving = true;
    try {
      await updateTraceSettings({ retentionDays });
    } catch {
      // silently ignore
    } finally {
      traceSaving = false;
    }
  }

  // =====================
  // Quit Application
  // =====================

  let showQuitConfirm = $state(false);

  async function handleQuit() {
    try {
      await invoke('window_close');
    } catch {
      // fallback — force exit
      try {
        const { getCurrentWindow } = await import('@tauri-apps/api/window');
        await getCurrentWindow().close();
      } catch {
        // last resort
      }
    }
  }

  // =====================
  // Section collapse
  // =====================

  let sections = $state({
    app: true,
    search: true,
    traces: true,
  });

  function toggleSection(key: keyof typeof sections) {
    sections[key] = !sections[key];
  }

  // Load on mount
  onMount(() => {
    loadAppSettings();
    loadSearchConfig();
  });
</script>

<div class="settings-tab">
  <p class="subtitle">App configuration and tools</p>

  <!-- 1. App Settings -->
  <div class="config-section">
    <button class="section-header" onclick={() => toggleSection('app')}>
      <div class="dot" style="background: #22c55e;"></div>
      <span class="section-label">Application</span>
      <span class="chevron" class:collapsed={!sections.app}></span>
    </button>
    {#if sections.app}
      <div class="section-body">
        <div class="toggle-row">
          <span>Minimize to Tray</span>
          <button
            class="toggle-switch"
            class:on={minimizeToTray}
            onclick={toggleMinimizeToTray}
            aria-label="Toggle minimize to tray"
          ></button>
        </div>
      </div>
    {/if}
  </div>

  <!-- 2. Web Search Config -->
  <div class="config-section">
    <button class="section-header" onclick={() => toggleSection('search')}>
      <div class="dot" style="background: #f59e0b;"></div>
      <span class="section-label">Web Search</span>
      <span class="chevron" class:collapsed={!sections.search}></span>
    </button>
    {#if sections.search}
      <div class="section-body">
        <div class="config-field">
          <label for="search-provider">Provider</label>
          <input
            id="search-provider"
            type="text"
            placeholder="e.g. brave, google"
            bind:value={searchConfig.provider}
            onchange={saveSearchConfig}
          />
        </div>
        <div class="config-field">
          <label for="search-endpoint">API Endpoint</label>
          <input
            id="search-endpoint"
            type="text"
            placeholder="https://api.search.brave.com/res/v1/web/search"
            bind:value={searchConfig.endpoint}
            onchange={saveSearchConfig}
          />
        </div>
      </div>
    {/if}
  </div>

  <!-- 3. Trace Settings -->
  <div class="config-section">
    <button class="section-header" onclick={() => toggleSection('traces')}>
      <div class="dot" style="background: #a855f7;"></div>
      <span class="section-label">Traces</span>
      <span class="chevron" class:collapsed={!sections.traces}></span>
    </button>
    {#if sections.traces}
      <div class="section-body">
        <div class="config-field">
          <label for="retention-days">Retention (days)</label>
          <input
            id="retention-days"
            type="number"
            min="1"
            max="365"
            style="width: 80px;"
            bind:value={retentionDays}
            onchange={saveTraceRetention}
          />
        </div>
        {#if traceSaving}
          <span class="save-indicator">Saving...</span>
        {/if}
      </div>
    {/if}
  </div>

  <!-- 4. Quit Application -->
  <div class="quit-section">
    {#if showQuitConfirm}
      <div class="quit-confirm">
        <p class="quit-confirm-text">Quit Holt? Sessions will be saved.</p>
        <div class="quit-confirm-actions">
          <button class="btn-danger" onclick={handleQuit}>Quit</button>
          <button class="btn-ghost" onclick={() => { showQuitConfirm = false; }}>Cancel</button>
        </div>
      </div>
    {:else}
      <button class="btn-quit" onclick={() => { showQuitConfirm = true; }}>
        Quit Holt
      </button>
    {/if}
  </div>
</div>

<style>
  .settings-tab {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .subtitle {
    font-size: 0.786rem;
    color: var(--text-muted, #64748b);
    margin-bottom: 12px;
  }

  /* Section structure */
  .config-section {
    margin-bottom: 12px;
  }

  .section-header {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    border: none;
    background: transparent;
    cursor: pointer;
    padding: 4px 0;
    margin-bottom: 8px;
  }

  .section-label {
    font-size: 0.714rem;
    text-transform: uppercase;
    letter-spacing: 1px;
    color: var(--text-muted, #64748b);
    flex: 1;
    text-align: left;
  }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .chevron {
    width: 0;
    height: 0;
    border-left: 4px solid transparent;
    border-right: 4px solid transparent;
    border-top: 5px solid var(--text-muted, #64748b);
    transition: transform 0.15s ease;
    flex-shrink: 0;
  }

  .chevron.collapsed {
    transform: rotate(-90deg);
  }

  .section-body {
    padding-left: 12px;
  }

  /* Toggle rows */
  .toggle-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 6px 0;
    font-size: 0.857rem;
    color: var(--text-primary, #e2e8f0);
    border-bottom: 1px solid var(--card-bg, #1e293b);
  }

  .toggle-switch {
    width: 32px;
    height: 18px;
    background: var(--surface-border, #334155);
    border-radius: 9px;
    position: relative;
    cursor: pointer;
    border: none;
    padding: 0;
    transition: background 0.15s;
    flex-shrink: 0;
  }

  .toggle-switch.on {
    background: var(--agent-accent, #06b6d4);
  }

  .toggle-switch::after {
    content: '';
    position: absolute;
    width: 14px;
    height: 14px;
    background: var(--text-primary, #e2e8f0);
    border-radius: 50%;
    top: 2px;
    left: 2px;
    transition: transform 0.15s;
  }

  .toggle-switch.on::after {
    transform: translateX(14px);
  }

  /* Config fields */
  .config-field {
    margin-bottom: 8px;
  }

  .config-field label {
    display: block;
    font-size: 0.786rem;
    color: var(--text-secondary, #94a3b8);
    margin-bottom: 3px;
  }

  .config-field input {
    width: 100%;
    background: var(--card-bg, #1e293b);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 6px;
    padding: 6px 8px;
    font-size: 0.857rem;
    color: var(--text-primary, #e2e8f0);
    font-family: inherit;
    outline: none;
    transition: border-color 0.15s;
    box-sizing: border-box;
  }

  .config-field input:focus {
    border-color: var(--agent-accent, #06b6d4);
  }

  .config-field input[type="number"] {
    appearance: textfield;
    -moz-appearance: textfield;
  }

  .save-indicator {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    font-style: italic;
  }

  /* Quit section */
  .quit-section {
    margin-top: 24px;
    padding-top: 16px;
    border-top: 1px solid var(--surface-border, #334155);
  }

  .btn-quit {
    width: 100%;
    padding: 8px;
    background: transparent;
    border: 1px solid var(--alert-accent, #ef4444);
    color: var(--alert-accent, #ef4444);
    border-radius: 8px;
    cursor: pointer;
    font-size: 0.857rem;
    transition: background 0.15s;
  }

  .btn-quit:hover {
    background: color-mix(in srgb, var(--alert-accent, #ef4444) 12%, transparent);
  }

  .quit-confirm {
    text-align: center;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .quit-confirm-text {
    font-size: 0.857rem;
    color: var(--text-primary, #e2e8f0);
  }

  .quit-confirm-actions {
    display: flex;
    gap: 8px;
    justify-content: center;
  }

  .btn-danger {
    background: var(--alert-accent, #ef4444);
    border: none;
    color: #ffffff;
    padding: 6px 16px;
    border-radius: 6px;
    font-size: 0.786rem;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .btn-danger:hover {
    opacity: 0.85;
  }

  .btn-ghost {
    background: transparent;
    border: 1px solid var(--surface-border, #334155);
    color: var(--text-muted, #64748b);
    padding: 6px 16px;
    border-radius: 6px;
    font-size: 0.786rem;
    cursor: pointer;
    transition: border-color 0.15s, color 0.15s;
  }

  .btn-ghost:hover {
    border-color: var(--text-muted, #64748b);
    color: var(--text-secondary, #94a3b8);
  }
</style>
