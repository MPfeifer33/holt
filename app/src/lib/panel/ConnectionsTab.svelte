<script lang="ts">
  import { onMount } from 'svelte';
  import {
    listConnections,
    addCloudApi,
    testConnection,
    removeConnection,
    scanLocalModels,
    storeApiKey,
    listAgents,
    type ConnectionInfo,
    type DetectedModel,
    type AgentSummary,
  } from '$lib/tauri/commands';
  import { TRUNC_CONNECTION } from '$lib/constants';

  // --- Section collapse state ---
  let sections = $state({
    apiKeys: true,
    cloudApis: true,
    localScanner: true,
    status: true,
  });

  function toggleSection(key: keyof typeof sections) {
    sections[key] = !sections[key];
  }

  // =====================
  // 1. API Keys Vault
  // =====================

  // Derive key names from connections (key_ref field)
  let keyRefs = $state<string[]>([]);
  let newKeyName = $state('');
  let newKeyValue = $state('');
  let keyStoreStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');
  let keyStoreError = $state<string | null>(null);
  let confirmDeleteKey = $state<string | null>(null);

  function maskKey(keyName: string): string {
    // Show as: keyname → sk-...xxxx (just masked placeholder since we can't read OS keychain)
    return `${keyName} → ••••••••`;
  }

  async function handleStoreKey() {
    if (!newKeyName.trim() || !newKeyValue.trim()) return;
    keyStoreStatus = 'saving';
    keyStoreError = null;
    try {
      await storeApiKey(newKeyName.trim(), newKeyValue.trim());
      // Add to local list if not already present
      if (!keyRefs.includes(newKeyName.trim())) {
        keyRefs = [...keyRefs, newKeyName.trim()];
      }
      keyStoreStatus = 'saved';
      newKeyName = '';
      newKeyValue = '';
      setTimeout(() => { keyStoreStatus = 'idle'; }, 2000);
    } catch (e) {
      keyStoreStatus = 'error';
      keyStoreError = e instanceof Error ? e.message : String(e);
    }
  }

  function promptDeleteKey(name: string) {
    confirmDeleteKey = name;
  }

  function cancelDeleteKey() {
    confirmDeleteKey = null;
  }

  async function confirmAndDeleteKey(name: string) {
    keyRefs = keyRefs.filter(k => k !== name);
    confirmDeleteKey = null;
    try {
      const { deleteApiKey } = await import('$lib/tauri/commands');
      await deleteApiKey(name);
    } catch (err) {
      console.error('Failed to delete API key from keychain:', err);
    }
  }

  // =====================
  // 2. Cloud API Connections
  // =====================

  let connections = $state<ConnectionInfo[]>([]);
  let connectionsLoading = $state(false);
  let connectionsError = $state<string | null>(null);

  // Per-connection test state
  let testStates = $state<Record<string, 'idle' | 'testing' | 'pass' | 'fail'>>({});
  // Last tested timestamps (component state, not persisted)
  let lastTested = $state<Record<string, string>>({});

  // Add connection form
  let showAddForm = $state(false);
  let addForm = $state({ provider: '', apiKey: '', endpoint: '' });
  let addStatus = $state<'idle' | 'saving' | 'error'>('idle');
  let addError = $state<string | null>(null);

  // Confirm remove
  let confirmRemoveConn = $state<string | null>(null);

  async function loadConnections() {
    connectionsLoading = true;
    connectionsError = null;
    try {
      const [conns, agents] = await Promise.all([
        listConnections(),
        listAgents(),
      ]);
      connections = conns;
      // Derive key refs from connections + agents for the vault
      const fromConns = conns.map(c => c.key_ref).filter(Boolean);
      const fromAgents = (agents as AgentSummary[]).map(() => '').filter(Boolean);
      const all = [...new Set([...fromConns, ...fromAgents])].filter(Boolean);
      // Merge into keyRefs (preserving any manually added ones)
      for (const ref of all) {
        if (!keyRefs.includes(ref)) {
          keyRefs = [...keyRefs, ref];
        }
      }
    } catch (e) {
      connectionsError = e instanceof Error ? e.message : String(e);
    } finally {
      connectionsLoading = false;
    }
  }

  async function handleTestConnection(keyRef: string) {
    testStates[keyRef] = 'testing';
    try {
      const ok = await testConnection(keyRef);
      testStates[keyRef] = ok ? 'pass' : 'fail';
      lastTested[keyRef] = new Date().toLocaleTimeString();
    } catch {
      testStates[keyRef] = 'fail';
      lastTested[keyRef] = new Date().toLocaleTimeString();
    }
  }

  async function handleTestAll() {
    const cloudConns = connections.filter(c => !c.is_local);
    await Promise.allSettled(cloudConns.map(c => handleTestConnection(c.key_ref)));
  }

  async function handleAddConnection() {
    if (!addForm.provider.trim() || !addForm.endpoint.trim()) return;
    addStatus = 'saving';
    addError = null;
    try {
      await addCloudApi(addForm.provider.trim(), addForm.apiKey.trim(), addForm.endpoint.trim());
      addForm = { provider: '', apiKey: '', endpoint: '' };
      showAddForm = false;
      addStatus = 'idle';
      await loadConnections();
    } catch (e) {
      addStatus = 'error';
      addError = e instanceof Error ? e.message : String(e);
    }
  }

  function promptRemoveConn(keyRef: string) {
    confirmRemoveConn = keyRef;
  }

  function cancelRemoveConn() {
    confirmRemoveConn = null;
  }

  async function confirmAndRemoveConn(keyRef: string) {
    confirmRemoveConn = null;
    try {
      await removeConnection(keyRef);
      await loadConnections();
    } catch {
      // silently ignore for now
    }
  }

  function truncate(str: string, max = TRUNC_CONNECTION): string {
    return str.length > max ? str.slice(0, max) + '…' : str;
  }

  // =====================
  // 3. Local Model Scanner
  // =====================

  let scanState = $state<'idle' | 'scanning' | 'done'>('idle');
  let scanResults = $state<DetectedModel[]>([]);
  let scanError = $state<string | null>(null);

  async function handleScan() {
    scanState = 'scanning';
    scanError = null;
    scanResults = [];
    try {
      scanResults = await scanLocalModels();
      scanState = 'done';
    } catch (e) {
      scanError = e instanceof Error ? e.message : String(e);
      scanState = 'done';
    }
  }

  // Load on mount
  onMount(() => {
    loadConnections();
  });
</script>

<div class="conn-sections">

  <!-- 1. API Keys Vault -->
  <div class="config-section">
    <button class="section-header" onclick={() => toggleSection('apiKeys')}>
      <div class="dot" style="background: #f59e0b;"></div>
      <span class="section-label">API Keys Vault</span>
      <span class="chevron" class:collapsed={!sections.apiKeys}></span>
    </button>
    {#if sections.apiKeys}
      <div class="section-body">
        {#if keyRefs.length === 0}
          <p class="hint">No keys stored yet. Add one below.</p>
        {:else}
          <ul class="key-list">
            {#each keyRefs as keyName (keyName)}
              <li class="key-row">
                <span class="key-name">{maskKey(keyName)}</span>
                {#if confirmDeleteKey === keyName}
                  <div class="confirm-row">
                    <span class="confirm-text">Delete?</span>
                    <button class="btn-danger-sm" onclick={() => confirmAndDeleteKey(keyName)}>Yes</button>
                    <button class="btn-ghost-sm" onclick={cancelDeleteKey}>No</button>
                  </div>
                {:else}
                  <button class="btn-ghost-sm" onclick={() => promptDeleteKey(keyName)} aria-label="Delete key {keyName}">✕</button>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}

        <div class="add-key-form">
          <div class="config-field">
            <label for="key-name">Key Name</label>
            <input
              id="key-name"
              type="text"
              placeholder="e.g. grok-api"
              bind:value={newKeyName}
            />
          </div>
          <div class="config-field">
            <label for="key-value">API Key</label>
            <input
              id="key-value"
              type="password"
              placeholder="sk-••••••••"
              bind:value={newKeyValue}
            />
          </div>
          {#if keyStoreStatus === 'error' && keyStoreError}
            <p class="error-text">{keyStoreError}</p>
          {/if}
          <div class="action-row">
            <button
              class="btn-primary"
              onclick={handleStoreKey}
              disabled={!newKeyName.trim() || !newKeyValue.trim() || keyStoreStatus === 'saving'}
            >
              {keyStoreStatus === 'saving' ? 'Storing…' : keyStoreStatus === 'saved' ? 'Stored ✓' : 'Store Key'}
            </button>
          </div>
        </div>
      </div>
    {/if}
  </div>

  <!-- 2. Cloud API Connections -->
  <div class="config-section">
    <button class="section-header" onclick={() => toggleSection('cloudApis')}>
      <div class="dot" style="background: #6366f1;"></div>
      <span class="section-label">Cloud API Connections</span>
      <span class="chevron" class:collapsed={!sections.cloudApis}></span>
    </button>
    {#if sections.cloudApis}
      <div class="section-body">
        {#if connectionsLoading}
          <p class="hint">Loading…</p>
        {:else if connectionsError}
          <p class="error-text">{connectionsError}</p>
        {:else if connections.filter(c => !c.is_local).length === 0 && !showAddForm}
          <p class="hint">No cloud connections configured.</p>
        {:else}
          <div class="conn-list">
            {#each connections.filter(c => !c.is_local) as conn (conn.id)}
              <div class="conn-row">
                <div class="conn-info">
                  <div class="conn-name-row">
                    <div
                      class="status-dot"
                      class:green={testStates[conn.key_ref] === 'pass'}
                      class:red={testStates[conn.key_ref] === 'fail'}
                    ></div>
                    <span class="conn-name">{conn.provider}</span>
                  </div>
                  <span class="conn-endpoint">{truncate(conn.endpoint)}</span>
                  {#if conn.key_ref}
                    <span class="conn-keyref">key: {conn.key_ref}</span>
                  {/if}
                  {#if lastTested[conn.key_ref]}
                    <span class="last-tested">tested {lastTested[conn.key_ref]}</span>
                  {/if}
                </div>
                <div class="conn-actions">
                  {#if confirmRemoveConn === conn.key_ref}
                    <span class="confirm-text">Remove?</span>
                    <button class="btn-danger-sm" onclick={() => confirmAndRemoveConn(conn.key_ref)}>Yes</button>
                    <button class="btn-ghost-sm" onclick={cancelRemoveConn}>No</button>
                  {:else}
                    <button
                      class="btn-accent-sm"
                      onclick={() => handleTestConnection(conn.key_ref)}
                      disabled={testStates[conn.key_ref] === 'testing'}
                    >
                      {testStates[conn.key_ref] === 'testing' ? '…' : 'Test'}
                    </button>
                    <button class="btn-ghost-sm" onclick={() => promptRemoveConn(conn.key_ref)} aria-label="Remove connection {conn.provider}">✕</button>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
        {/if}

        {#if showAddForm}
          <div class="add-conn-form">
            <div class="config-field">
              <label for="conn-provider">Provider Name</label>
              <input id="conn-provider" type="text" placeholder="e.g. Grok" bind:value={addForm.provider} />
            </div>
            <div class="config-field">
              <label for="conn-endpoint">Endpoint URL</label>
              <input id="conn-endpoint" type="text" placeholder="https://api.x.ai/v1" bind:value={addForm.endpoint} />
            </div>
            <div class="config-field">
              <label for="conn-apikey">API Key</label>
              <input id="conn-apikey" type="password" placeholder="sk-••••••••" bind:value={addForm.apiKey} />
            </div>
            {#if addStatus === 'error' && addError}
              <p class="error-text">{addError}</p>
            {/if}
            <div class="action-row">
              <button
                class="btn-primary"
                onclick={handleAddConnection}
                disabled={!addForm.provider.trim() || !addForm.endpoint.trim() || addStatus === 'saving'}
              >
                {addStatus === 'saving' ? 'Adding…' : 'Add Connection'}
              </button>
              <button class="btn-ghost" onclick={() => { showAddForm = false; addForm = { provider: '', apiKey: '', endpoint: '' }; addStatus = 'idle'; }}>
                Cancel
              </button>
            </div>
          </div>
        {:else}
          <div class="action-row" style="margin-top: 8px;">
            <button class="btn-ghost" onclick={() => { showAddForm = true; }}>+ Add Connection</button>
          </div>
        {/if}
      </div>
    {/if}
  </div>

  <!-- 3. Local Model Scanner -->
  <div class="config-section">
    <button class="section-header" onclick={() => toggleSection('localScanner')}>
      <div class="dot" style="background: #22c55e;"></div>
      <span class="section-label">Local Model Scanner</span>
      <span class="chevron" class:collapsed={!sections.localScanner}></span>
    </button>
    {#if sections.localScanner}
      <div class="section-body">
        <p class="hint">Scan for Ollama, LM Studio, llama-server on common local ports.</p>
        <div class="action-row" style="margin-bottom: 10px;">
          <button
            class="btn-accent"
            onclick={handleScan}
            disabled={scanState === 'scanning'}
            aria-label="Scan for local models"
          >
            {#if scanState === 'scanning'}
              <span class="spinner"></span> Scanning…
            {:else}
              Scan for Local Models
            {/if}
          </button>
        </div>

        {#if scanError}
          <p class="error-text">{scanError}</p>
        {/if}

        {#if scanState === 'done'}
          {#if scanResults.length === 0}
            <p class="hint">No local model servers found.</p>
          {:else}
            <div class="scan-results">
              {#each scanResults as result (result.endpoint)}
                <div class="scan-result-row">
                  <div class="scan-service-header">
                    <div class="status-dot green"></div>
                    <span class="scan-service-name">{result.service_name}</span>
                    <span class="scan-endpoint">{result.endpoint}</span>
                    <span class="scan-port">:{result.port}</span>
                  </div>
                  {#if result.models.length > 0}
                    <ul class="model-list">
                      {#each result.models as model}
                        <li class="model-item">{model}</li>
                      {/each}
                    </ul>
                  {:else}
                    <p class="hint" style="margin: 2px 0 0 12px;">No models loaded</p>
                  {/if}
                </div>
              {/each}
            </div>
          {/if}
        {/if}
      </div>
    {/if}
  </div>

  <!-- 4. Connection Status -->
  <div class="config-section">
    <button class="section-header" onclick={() => toggleSection('status')}>
      <div class="dot" style="background: #06b6d4;"></div>
      <span class="section-label">Connection Status</span>
      <span class="chevron" class:collapsed={!sections.status}></span>
    </button>
    {#if sections.status}
      <div class="section-body">
        {#if connections.length === 0}
          <p class="hint">No connections configured.</p>
        {:else}
          <div class="status-list">
            {#each connections as conn (conn.id)}
              <div class="status-row">
                <div
                  class="status-dot"
                  class:green={testStates[conn.key_ref] === 'pass'}
                  class:red={testStates[conn.key_ref] === 'fail'}
                ></div>
                <div class="status-info">
                  <span class="status-name">{conn.provider}</span>
                  {#if conn.is_local}
                    <span class="status-badge local">local</span>
                  {:else}
                    <span class="status-badge cloud">cloud</span>
                  {/if}
                </div>
                <div class="status-meta">
                  {#if testStates[conn.key_ref] === 'testing'}
                    <span class="testing-text">testing…</span>
                  {:else if testStates[conn.key_ref] === 'pass'}
                    <span class="pass-text">OK</span>
                  {:else if testStates[conn.key_ref] === 'fail'}
                    <span class="fail-text">Failed</span>
                  {:else}
                    <span class="untested-text">—</span>
                  {/if}
                  {#if lastTested[conn.key_ref]}
                    <span class="timestamp">{lastTested[conn.key_ref]}</span>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
          <div class="action-row" style="margin-top: 10px;">
            <button class="btn-ghost" onclick={handleTestAll}>Test All</button>
          </div>
        {/if}
      </div>
    {/if}
  </div>

</div>

<style>
  .conn-sections {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  /* Section structure — matches AgentTab pattern */
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

  /* Fields */
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

  /* Hints and errors */
  .hint {
    font-size: 0.786rem;
    color: var(--text-muted, #64748b);
    margin: 0 0 8px 0;
    line-height: 1.4;
  }

  .error-text {
    font-size: 0.786rem;
    color: var(--alert-red, #ef4444);
    margin: 4px 0 8px 0;
  }

  /* Action rows */
  .action-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  /* Buttons */
  .btn-primary {
    background: var(--agent-accent, #06b6d4);
    border: none;
    color: #0f172a;
    padding: 5px 14px;
    border-radius: 6px;
    font-size: 0.786rem;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .btn-primary:hover:not(:disabled) {
    opacity: 0.85;
  }

  .btn-primary:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .btn-accent {
    background: transparent;
    border: 1px solid var(--agent-accent, #06b6d4);
    color: var(--agent-accent, #06b6d4);
    padding: 5px 14px;
    border-radius: 6px;
    font-size: 0.786rem;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 6px;
    transition: background 0.15s;
  }

  .btn-accent:hover:not(:disabled) {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 12%, transparent);
  }

  .btn-accent:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .btn-accent-sm {
    background: transparent;
    border: 1px solid var(--agent-accent, #06b6d4);
    color: var(--agent-accent, #06b6d4);
    padding: 2px 8px;
    border-radius: 4px;
    font-size: 0.714rem;
    cursor: pointer;
    transition: background 0.15s;
    flex-shrink: 0;
  }

  .btn-accent-sm:hover:not(:disabled) {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 12%, transparent);
  }

  .btn-accent-sm:disabled {
    opacity: 0.5;
    cursor: default;
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

  .btn-ghost-sm {
    background: transparent;
    border: none;
    color: var(--text-muted, #64748b);
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 0.786rem;
    cursor: pointer;
    transition: color 0.15s;
    flex-shrink: 0;
  }

  .btn-ghost-sm:hover {
    color: var(--text-secondary, #94a3b8);
  }

  .btn-danger-sm {
    background: transparent;
    border: 1px solid var(--alert-red, #ef4444);
    color: var(--alert-red, #ef4444);
    padding: 2px 8px;
    border-radius: 4px;
    font-size: 0.714rem;
    cursor: pointer;
    flex-shrink: 0;
  }

  /* Status dot */
  .status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--surface-border, #334155);
    flex-shrink: 0;
    transition: background 0.2s;
  }

  .status-dot.green {
    background: var(--success-green, #22c55e);
    box-shadow: 0 0 4px color-mix(in srgb, #22c55e 60%, transparent);
  }

  .status-dot.red {
    background: var(--alert-red, #ef4444);
  }

  /* API Keys list */
  .key-list {
    list-style: none;
    padding: 0;
    margin: 0 0 10px 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .key-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 8px;
    background: var(--card-bg, #1e293b);
    border-radius: 6px;
    border: 1px solid var(--surface-border, #334155);
  }

  .key-name {
    font-size: 0.786rem;
    color: var(--text-secondary, #94a3b8);
    font-family: var(--mono-family, 'JetBrains Mono', monospace);
    flex: 1;
  }

  .confirm-row {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .confirm-text {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
  }

  /* Add key form */
  .add-key-form {
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid var(--card-bg, #1e293b);
  }

  /* Cloud connections list */
  .conn-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 8px;
  }

  .conn-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 8px;
    padding: 8px;
    background: var(--card-bg, #1e293b);
    border-radius: 6px;
    border: 1px solid var(--surface-border, #334155);
  }

  .conn-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }

  .conn-name-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .conn-name {
    font-size: 0.857rem;
    font-weight: 600;
    color: var(--text-primary, #e2e8f0);
  }

  .conn-endpoint {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    font-family: var(--mono-family, 'JetBrains Mono', monospace);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .conn-keyref {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
  }

  .last-tested {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    font-style: italic;
  }

  .conn-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  /* Add connection form */
  .add-conn-form {
    margin-top: 10px;
    padding: 10px;
    background: var(--card-bg, #1e293b);
    border-radius: 6px;
    border: 1px solid var(--surface-border, #334155);
  }

  /* Local scanner */
  .spinner {
    display: inline-block;
    width: 10px;
    height: 10px;
    border: 2px solid currentColor;
    border-top-color: transparent;
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .scan-results {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 8px;
  }

  .scan-result-row {
    padding: 8px;
    background: var(--card-bg, #1e293b);
    border-radius: 6px;
    border: 1px solid var(--surface-border, #334155);
  }

  .scan-service-header {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 4px;
  }

  .scan-service-name {
    font-size: 0.857rem;
    font-weight: 600;
    color: var(--text-primary, #e2e8f0);
  }

  .scan-endpoint {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    font-family: var(--mono-family, 'JetBrains Mono', monospace);
  }

  .scan-port {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
    font-family: var(--mono-family, 'JetBrains Mono', monospace);
  }

  .model-list {
    list-style: none;
    padding: 0 0 0 14px;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .model-item {
    font-size: 0.714rem;
    color: var(--text-secondary, #94a3b8);
    font-family: var(--mono-family, 'JetBrains Mono', monospace);
  }

  .model-item::before {
    content: '• ';
    color: var(--text-muted, #64748b);
  }

  /* Status section */
  .status-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .status-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 0;
    border-bottom: 1px solid var(--card-bg, #1e293b);
  }

  .status-info {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1;
  }

  .status-name {
    font-size: 0.857rem;
    color: var(--text-primary, #e2e8f0);
  }

  .status-badge {
    font-size: 0.643rem;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    padding: 1px 5px;
    border-radius: 3px;
  }

  .status-badge.cloud {
    background: color-mix(in srgb, #6366f1 15%, transparent);
    color: #6366f1;
    border: 1px solid #6366f1;
  }

  .status-badge.local {
    background: color-mix(in srgb, #22c55e 15%, transparent);
    color: #22c55e;
    border: 1px solid #22c55e;
  }

  .status-meta {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 1px;
  }

  .pass-text {
    font-size: 0.714rem;
    color: var(--success-green, #22c55e);
    font-weight: 600;
  }

  .fail-text {
    font-size: 0.714rem;
    color: var(--alert-red, #ef4444);
    font-weight: 600;
  }

  .testing-text {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
  }

  .untested-text {
    font-size: 0.714rem;
    color: var(--text-muted, #64748b);
  }

  .timestamp {
    font-size: 0.643rem;
    color: var(--text-muted, #64748b);
  }
</style>
