<script lang="ts">
  import { THEMES } from '$lib/theme/themes';
  import {
    getActiveThemeId,
    setTheme,
    getActiveTheme,
    getFontScale,
    setFontScale,
    getCustomFontFamily,
    setCustomFontFamily,
    getCustomFontSize,
    setCustomFontSize,
    FONT_OPTIONS,
  } from '$lib/stores/theme.svelte';
  import { STORAGE_KEY_CUSTOM_THEME } from '$lib/constants';

  // --- Theme presets ---

  const themeKeys = Object.keys(THEMES) as string[];

  // --- Custom color overrides ---

  let customColors = $state<Record<string, string>>({});
  let customInitialized = $state(false);

  function initCustomColors() {
    const theme = getActiveTheme();
    customColors = {
      infraAccent: theme.infraAccent,
      agentAccent: theme.agentAccent,
      alertAccent: theme.alertAccent,
      successAccent: theme.successAccent,
      warningAccent: theme.warningAccent,
      canvasBg: theme.canvasBg,
      cardBg: theme.cardBg,
      textPrimary: theme.textPrimary,
    };
    customInitialized = true;
  }

  $effect(() => {
    if (!customInitialized) {
      initCustomColors();
    }
  });

  function selectPreset(id: string) {
    setTheme(id);
    const theme = getActiveTheme();
    customColors = {
      infraAccent: theme.infraAccent,
      agentAccent: theme.agentAccent,
      alertAccent: theme.alertAccent,
      successAccent: theme.successAccent,
      warningAccent: theme.warningAccent,
      canvasBg: theme.canvasBg,
      cardBg: theme.cardBg,
      textPrimary: theme.textPrimary,
    };
    // Clear saved overrides when switching presets
    try { localStorage.removeItem(STORAGE_KEY_CUSTOM_THEME); } catch {}
  }

  function applyCustomColor(key: string, value: string) {
    customColors[key] = value;
    const cssMap: Record<string, string> = {
      infraAccent: '--infra-accent',
      agentAccent: '--agent-accent',
      alertAccent: '--alert-accent',
      successAccent: '--success-accent',
      warningAccent: '--warning-accent',
      canvasBg: '--canvas-bg',
      cardBg: '--card-bg',
      textPrimary: '--text-primary',
    };
    const cssProp = cssMap[key];
    if (cssProp) {
      document.documentElement.style.setProperty(cssProp, value);
    }
    saveCustomTheme();
  }

  function saveCustomTheme() {
    try {
      localStorage.setItem(STORAGE_KEY_CUSTOM_THEME, JSON.stringify(customColors));
    } catch {}
  }

  function loadCustomTheme() {
    try {
      const saved = localStorage.getItem(STORAGE_KEY_CUSTOM_THEME);
      if (saved) {
        const parsed = JSON.parse(saved);
        customColors = { ...customColors, ...parsed };
        // Apply saved overrides to the DOM
        for (const [key, value] of Object.entries(parsed)) {
          applyCustomColor(key, value as string);
        }
      }
    } catch {}
  }

  const COLOR_FIELDS: { key: string; label: string }[] = [
    { key: 'infraAccent', label: 'Infrastructure' },
    { key: 'agentAccent', label: 'Agent' },
    { key: 'alertAccent', label: 'Alert' },
    { key: 'successAccent', label: 'Success' },
    { key: 'warningAccent', label: 'Warning' },
    { key: 'canvasBg', label: 'Canvas' },
    { key: 'cardBg', label: 'Card' },
    { key: 'textPrimary', label: 'Text' },
  ];

  // --- Font ---

  let currentScale = $derived(getFontScale());

  // --- Sections ---

  let sections = $state({ presets: true, customize: false, font: true });

  function toggleSection(key: keyof typeof sections) {
    sections[key] = !sections[key];
  }

  $effect(() => {
    loadCustomTheme();
  });
</script>

<div class="theme-tab">
  <p class="subtitle">Appearance, fonts, and display scaling</p>

  <!-- 1. Theme Presets -->
  <div class="config-section">
    <button class="section-header" onclick={() => toggleSection('presets')}>
      <div class="dot" style="background: #a855f7;"></div>
      <span class="section-label">Themes</span>
      <span class="chevron" class:collapsed={!sections.presets}></span>
    </button>
    {#if sections.presets}
      <div class="section-body">
        <div class="theme-grid">
          {#each themeKeys as id (id)}
            {@const theme = THEMES[id]}
            <button
              class="theme-card"
              class:active={getActiveThemeId() === id}
              style="
                background: {theme.canvasBg};
                border-color: {getActiveThemeId() === id ? theme.agentAccent : theme.surfaceBorder};
                border-radius: {theme.borderRadius};
              "
              onclick={() => selectPreset(id)}
            >
              <div class="theme-swatches">
                <span class="swatch" style="background: {theme.infraAccent};"></span>
                <span class="swatch" style="background: {theme.agentAccent};"></span>
                <span class="swatch" style="background: {theme.alertAccent};"></span>
                <span class="swatch" style="background: {theme.successAccent};"></span>
              </div>
              <span
                class="theme-card-name"
                style="
                  color: {theme.textPrimary};
                  font-family: {theme.fontFamily};
                  text-transform: {theme.textTransform};
                  letter-spacing: {theme.textTransform === 'uppercase' ? '1px' : '0'};
                "
              >{theme.name}</span>
              {#if getActiveThemeId() === id}
                <span class="theme-active-badge" style="color: {theme.agentAccent};">Active</span>
              {/if}
            </button>
          {/each}
        </div>
      </div>
    {/if}
  </div>

  <!-- 2. Customize Colors -->
  <div class="config-section">
    <button class="section-header" onclick={() => toggleSection('customize')}>
      <div class="dot" style="background: #f59e0b;"></div>
      <span class="section-label">Customize Colors</span>
      <span class="chevron" class:collapsed={!sections.customize}></span>
    </button>
    {#if sections.customize}
      <div class="section-body">
        <div class="color-grid">
          {#each COLOR_FIELDS as field (field.key)}
            <span class="color-label">{field.label}</span>
            <div class="color-picker-row">
              <input
                type="color"
                class="color-input"
                value={customColors[field.key] || '#000000'}
                oninput={(e) => applyCustomColor(field.key, (e.target as HTMLInputElement).value)}
              />
              <span class="color-hex">{customColors[field.key] || ''}</span>
            </div>
          {/each}
        </div>
      </div>
    {/if}
  </div>

  <!-- 3. Font & Scale -->
  <div class="config-section">
    <button class="section-header" onclick={() => toggleSection('font')}>
      <div class="dot" style="background: #06b6d4;"></div>
      <span class="section-label">Font & Scale</span>
      <span class="chevron" class:collapsed={!sections.font}></span>
    </button>
    {#if sections.font}
      <div class="section-body">
        <!-- Font Family -->
        <div class="font-field">
          <label class="font-field-label" for="font-family-select">Font Family</label>
          <select
            id="font-family-select"
            class="font-select"
            value={getCustomFontFamily() || ''}
            onchange={(e) => setCustomFontFamily((e.target as HTMLSelectElement).value)}
          >
            {#each FONT_OPTIONS as opt}
              <option value={opt.value}>{opt.label}</option>
            {/each}
          </select>
        </div>

        <!-- Font Size -->
        <div class="font-field">
          <label class="font-field-label" for="font-size-input">Font Size</label>
          <div class="font-size-row">
            <input
              id="font-size-input"
              type="number"
              class="font-size-input"
              min="8"
              max="32"
              step="1"
              placeholder="14"
              value={getCustomFontSize() ?? ''}
              onchange={(e) => {
                const val = (e.target as HTMLInputElement).value;
                if (val === '') {
                  setCustomFontSize(null);
                } else {
                  setCustomFontSize(parseInt(val, 10));
                }
              }}
            />
            <span class="font-size-unit">px</span>
            {#if getCustomFontSize() !== null}
              <button class="font-size-reset" onclick={() => setCustomFontSize(null)}>Reset</button>
            {/if}
          </div>
        </div>

        <!-- Legacy scale slider (secondary control) -->
        {#if getCustomFontSize() === null}
          <div class="font-scale">
            <div class="scale-label">
              <span>Display Scale</span>
              <span class="scale-value">{Math.round(currentScale * 100)}%</span>
            </div>
            <div class="scale-row">
              <span class="scale-bound">80%</span>
              <input
                type="range"
                min="0.8"
                max="1.5"
                step="0.05"
                value={currentScale}
                oninput={(e) => setFontScale(parseFloat((e.target as HTMLInputElement).value))}
                class="scale-slider"
              />
              <span class="scale-bound">150%</span>
            </div>
            {#if currentScale !== 1.0}
              <button class="scale-reset" onclick={() => setFontScale(1.0)}>Reset to 100%</button>
            {/if}
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .theme-tab {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .subtitle {
    font-size: 0.786rem;
    color: var(--text-muted);
    margin-bottom: 4px;
  }

  /* Config sections */
  .config-section {
    display: flex;
    flex-direction: column;
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    overflow: hidden;
  }

  .section-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    background: var(--card-bg);
    border: none;
    cursor: pointer;
    color: var(--text-primary);
    font-size: 0.857rem;
    font-weight: 600;
    width: 100%;
    text-align: left;
  }

  .section-header:hover {
    background: color-mix(in srgb, var(--card-bg) 80%, var(--surface-border));
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .section-label {
    flex: 1;
  }

  .chevron {
    font-size: 0.714rem;
    transition: transform 0.15s;
    color: var(--text-muted);
  }

  .chevron::after {
    content: '\25BC';
  }

  .chevron.collapsed {
    transform: rotate(-90deg);
  }

  .section-body {
    padding: 10px;
    background: var(--canvas-bg);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  /* Theme grid */
  .theme-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
  }

  .theme-card {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 12px 8px 10px;
    border: 2px solid;
    cursor: pointer;
    transition: border-color 0.2s, transform 0.15s, box-shadow 0.2s;
  }

  .theme-card:hover {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
  }

  .theme-card.active {
    box-shadow: 0 0 12px rgba(99, 102, 241, 0.2);
  }

  .theme-swatches {
    display: flex;
    gap: 4px;
  }

  .swatch {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 1px solid rgba(255, 255, 255, 0.1);
  }

  .theme-card-name {
    font-size: 0.786rem;
    font-weight: 600;
  }

  .theme-active-badge {
    position: absolute;
    top: 4px;
    right: 6px;
    font-size: 0.571rem;
    font-family: var(--mono-family, monospace);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-weight: 700;
  }

  /* Color customization */
  .color-grid {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 6px 10px;
    align-items: center;
  }

  .color-label {
    font-size: 0.786rem;
    color: var(--text-secondary);
  }

  .color-picker-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .color-input {
    width: 28px;
    height: 22px;
    border: var(--border-width) solid var(--surface-border);
    border-radius: 4px;
    padding: 1px;
    cursor: pointer;
    background: transparent;
  }

  .color-input::-webkit-color-swatch-wrapper { padding: 0; }
  .color-input::-webkit-color-swatch {
    border: none;
    border-radius: 3px;
  }

  .color-hex {
    color: var(--text-muted);
    font-family: var(--mono-family, monospace);
    font-size: 0.714rem;
  }

  /* Font fields */
  .font-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 8px;
  }

  .font-field-label {
    font-size: 0.786rem;
    color: var(--text-secondary);
    font-weight: 500;
  }

  .font-select {
    font-size: 0.857rem;
    padding: 5px 8px;
    border-radius: 6px;
    border: 1px solid var(--surface-border);
    background: var(--card-bg);
    color: var(--text-primary);
    cursor: pointer;
    outline: none;
    transition: border-color 0.15s;
  }

  .font-select:focus {
    border-color: var(--agent-accent);
  }

  .font-size-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .font-size-input {
    width: 60px;
    font-size: 0.857rem;
    padding: 5px 8px;
    border-radius: 6px;
    border: 1px solid var(--surface-border);
    background: var(--card-bg);
    color: var(--text-primary);
    outline: none;
    transition: border-color 0.15s;
    font-family: var(--mono-family, monospace);
  }

  .font-size-input:focus {
    border-color: var(--agent-accent);
  }

  .font-size-input::placeholder {
    color: var(--text-muted);
  }

  .font-size-unit {
    font-size: 0.786rem;
    color: var(--text-muted);
    font-family: var(--mono-family, monospace);
  }

  .font-size-reset {
    font-size: 0.714rem;
    font-family: var(--mono-family);
    color: var(--text-muted);
    background: none;
    border: none;
    cursor: pointer;
    padding: 2px 4px;
  }

  .font-size-reset:hover {
    color: var(--agent-accent);
  }

  /* Font scale */
  .font-scale {
    margin-top: 6px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .scale-label {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 0.857rem;
    color: var(--text-primary);
  }

  .scale-value {
    font-family: var(--mono-family);
    font-size: 0.786rem;
    color: var(--agent-accent);
    font-weight: 600;
  }

  .scale-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .scale-bound {
    font-size: 0.714rem;
    font-family: var(--mono-family);
    color: var(--text-muted);
    flex-shrink: 0;
    width: 32px;
    text-align: center;
  }

  .scale-slider {
    flex: 1;
    -webkit-appearance: none;
    appearance: none;
    height: 4px;
    background: var(--surface-border);
    border-radius: 2px;
    outline: none;
    cursor: pointer;
  }

  .scale-slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--agent-accent);
    border: 2px solid var(--card-bg);
    cursor: pointer;
  }

  .scale-reset {
    align-self: flex-end;
    font-size: 0.714rem;
    font-family: var(--mono-family);
    color: var(--text-muted);
    background: none;
    border: none;
    cursor: pointer;
    padding: 2px 0;
  }

  .scale-reset:hover {
    color: var(--agent-accent);
  }
</style>
