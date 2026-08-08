<script lang="ts">
  import { onMount } from 'svelte';
  import {
    listPlugins,
    enablePlugin,
    disablePlugin,
    restartPlugin,
    type PluginInfo,
  } from '$lib/tauri/commands';

  let plugins = $state<PluginInfo[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let expandedId = $state<string | null>(null);
  let actionInProgress = $state<Record<string, boolean>>({});

  async function loadPlugins() {
    loading = true;
    error = null;
    try {
      plugins = await listPlugins();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function healthColor(health: string): string {
    switch (health.toLowerCase()) {
      case 'healthy': return 'var(--success-accent, #22c55e)';
      case 'degraded': return 'var(--warning-accent, #f59e0b)';
      case 'unhealthy':
      case 'error':
      case 'failed': return 'var(--alert-accent, #ef4444)';
      default: return 'var(--text-muted, #64748b)';
    }
  }

  async function handleToggle(plugin: PluginInfo) {
    actionInProgress[plugin.id] = true;
    try {
      if (plugin.enabled) {
        await disablePlugin(plugin.id);
      } else {
        await enablePlugin(plugin.id);
      }
      await loadPlugins();
    } catch {
      // reload to get actual state
      await loadPlugins();
    } finally {
      actionInProgress[plugin.id] = false;
    }
  }

  async function handleRestart(pluginId: string) {
    actionInProgress[pluginId] = true;
    try {
      await restartPlugin(pluginId);
      await loadPlugins();
    } catch {
      await loadPlugins();
    } finally {
      actionInProgress[pluginId] = false;
    }
  }

  function toggleExpand(pluginId: string) {
    expandedId = expandedId === pluginId ? null : pluginId;
  }

  // Load on mount
  onMount(() => {
    loadPlugins();
  });
</script>

<div class="plugins-tab">
  <p class="subtitle">Manage installed plugins</p>

  {#if loading}
    <p class="hint">Loading plugins...</p>
  {:else if error}
    <p class="error-text">{error}</p>
    <button class="btn-ghost" onclick={loadPlugins}>Retry</button>
  {:else if plugins.length === 0}
    <div class="empty-state">
      <p class="hint">No plugins installed.</p>
      <p class="hint">Plugins are configured via config.toml</p>
    </div>
  {:else}
    <div class="plugin-list">
      {#each plugins as plugin (plugin.id)}
        <div class="plugin-row">
          <div
            class="plugin-header"
            onclick={() => toggleExpand(plugin.id)}
            onkeydown={(e) => e.key === 'Enter' && toggleExpand(plugin.id)}
            role="button"
            tabindex="0"
          >
            <div class="plugin-health-dot" style="background: {healthColor(plugin.health)};"></div>
            <div class="plugin-info">
              <span class="plugin-name">{plugin.name}</span>
              <span class="plugin-version">v{plugin.version}</span>
            </div>
            <div class="plugin-controls">
              <button
                class="toggle-switch"
                class:on={plugin.enabled}
                onclick={(e) => { e.stopPropagation(); handleToggle(plugin); }}
                disabled={actionInProgress[plugin.id]}
                aria-label={plugin.enabled ? 'Disable plugin' : 'Enable plugin'}
              ></button>
              <button
                class="btn-restart"
                onclick={(e) => { e.stopPropagation(); handleRestart(plugin.id); }}
                disabled={actionInProgress[plugin.id] || !plugin.enabled}
                aria-label="Restart plugin"
                title="Restart"
              >
                &#x21bb;
              </button>
            </div>
          </div>

          {#if expandedId === plugin.id}
            <div class="plugin-details">
              <div class="detail-row">
                <span class="detail-label">Status</span>
                <span class="detail-value">{plugin.status}</span>
              </div>
              <div class="detail-row">
                <span class="detail-label">Health</span>
                <span class="detail-value" style="color: {healthColor(plugin.health)};">{plugin.health}</span>
              </div>
              {#if plugin.health_message}
                <div class="detail-row">
                  <span class="detail-label">Message</span>
                  <span class="detail-value detail-message">{plugin.health_message}</span>
                </div>
              {/if}
              <div class="detail-row">
                <span class="detail-label">Tools</span>
                <span class="detail-value">{plugin.tool_count}</span>
              </div>
              <div class="detail-row">
                <span class="detail-label">ID</span>
                <span class="detail-value mono">{plugin.id}</span>
              </div>
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .plugins-tab {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .subtitle {
    font-size: 0.786rem;
    color: var(--text-muted, #64748b);
    margin-bottom: 12px;
  }

  .hint {
    font-size: 0.786rem;
    color: var(--text-muted, #64748b);
    margin: 0;
    line-height: 1.4;
  }

  .error-text {
    font-size: 0.786rem;
    color: var(--alert-accent, #ef4444);
    margin: 0 0 8px 0;
  }

  .empty-state {
    text-align: center;
    padding: 40px 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .plugin-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .plugin-row {
    background: var(--card-bg, #1e293b);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 6px;
    overflow: hidden;
  }

  .plugin-header {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px;
    border: none;
    background: transparent;
    cursor: pointer;
    text-align: left;
  }

  .plugin-header:hover {
    background: color-mix(in srgb, var(--surface-border, #334155) 30%, transparent);
  }

  .plugin-health-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .plugin-info {
    flex: 1;
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
  }

  .plugin-name {
    font-size: 0.857rem;
    font-weight: 600;
    color: var(--text-primary, #e2e8f0);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .plugin-version {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    font-family: var(--mono-family, 'JetBrains Mono', monospace);
    flex-shrink: 0;
  }

  .plugin-controls {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  /* Toggle switch */
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

  .toggle-switch:disabled {
    opacity: 0.5;
    cursor: default;
  }

  /* Restart button */
  .btn-restart {
    width: 24px;
    height: 24px;
    border: 1px solid var(--surface-border, #334155);
    background: transparent;
    color: var(--text-muted, #64748b);
    border-radius: 4px;
    font-size: 1rem;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    transition: border-color 0.15s, color 0.15s;
  }

  .btn-restart:hover:not(:disabled) {
    border-color: var(--agent-accent, #06b6d4);
    color: var(--agent-accent, #06b6d4);
  }

  .btn-restart:disabled {
    opacity: 0.3;
    cursor: default;
  }

  /* Expanded details */
  .plugin-details {
    padding: 0 10px 10px 10px;
    border-top: 1px solid var(--surface-border, #334155);
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-top: 8px;
  }

  .detail-row {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 8px;
  }

  .detail-label {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    flex-shrink: 0;
  }

  .detail-value {
    font-size: 0.786rem;
    color: var(--text-secondary, #94a3b8);
    text-align: right;
    word-break: break-all;
  }

  .detail-value.mono {
    font-family: var(--mono-family, 'JetBrains Mono', monospace);
    font-size: 0.714rem;
  }

  .detail-message {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    max-width: 160px;
  }

  .btn-ghost {
    background: transparent;
    border: 1px solid var(--surface-border, #334155);
    color: var(--text-muted, #64748b);
    padding: 5px 12px;
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
