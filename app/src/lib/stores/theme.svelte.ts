import { THEMES, DEFAULT_THEME, type Theme } from '$lib/theme/themes';

const STORAGE_KEY_THEME = 'holt-theme-id';
const STORAGE_KEY_FONT_MODE = 'holt-font-mode';
const STORAGE_KEY_FONT_SCALE = 'holt-font-scale';
const STORAGE_KEY_FONT_FAMILY = 'holt-font-family';
const STORAGE_KEY_FONT_SIZE = 'holt-font-size-px';
const STORAGE_KEY_CUSTOM_THEME = 'holt-custom-theme';

let activeThemeId = $state(DEFAULT_THEME);
let fontMode = $state<'sans' | 'serif'>('sans');
let fontScale = $state(1.0);
let customFontFamily = $state<string | null>(null);
let customFontSize = $state<number | null>(null);

/** Available font families for the dropdown. */
export const FONT_OPTIONS = [
  { label: 'Theme Default', value: '' },
  { label: 'Inter', value: 'Inter, sans-serif' },
  { label: 'JetBrains Mono', value: 'JetBrains Mono, monospace' },
  { label: 'Fira Code', value: 'Fira Code, monospace' },
  { label: 'IBM Plex Sans', value: 'IBM Plex Sans, sans-serif' },
  { label: 'IBM Plex Mono', value: 'IBM Plex Mono, monospace' },
  { label: 'Source Code Pro', value: 'Source Code Pro, monospace' },
  { label: 'Roboto', value: 'Roboto, sans-serif' },
  { label: 'Noto Sans', value: 'Noto Sans, sans-serif' },
  { label: 'Ubuntu', value: 'Ubuntu, sans-serif' },
  { label: 'Ubuntu Mono', value: 'Ubuntu Mono, monospace' },
  { label: 'System UI', value: 'system-ui, sans-serif' },
];

export function getActiveTheme(): Theme {
  return THEMES[activeThemeId] ?? THEMES[DEFAULT_THEME];
}

export function getActiveThemeId(): string {
  return activeThemeId;
}

export function setTheme(id: string): void {
  if (THEMES[id]) {
    activeThemeId = id;
    applyTheme();
    save(STORAGE_KEY_THEME, id);
  }
}

export function getFontMode(): 'sans' | 'serif' {
  return fontMode;
}

export function setFontMode(mode: 'sans' | 'serif'): void {
  fontMode = mode;
  applyTheme();
  save(STORAGE_KEY_FONT_MODE, mode);
}

export function getFontScale(): number {
  return fontScale;
}

export function setFontScale(scale: number): void {
  fontScale = Math.max(0.8, Math.min(1.5, scale));
  applyFontScale();
  save(STORAGE_KEY_FONT_SCALE, String(fontScale));
}

export function getCustomFontFamily(): string | null {
  return customFontFamily;
}

export function setCustomFontFamily(family: string): void {
  customFontFamily = family || null;
  applyFontFamily();
  save(STORAGE_KEY_FONT_FAMILY, family);
}

export function getCustomFontSize(): number | null {
  return customFontSize;
}

export function setCustomFontSize(size: number | null): void {
  if (size !== null) {
    size = Math.max(8, Math.min(32, size));
  }
  customFontSize = size;
  applyFontScale();
  if (size !== null) {
    save(STORAGE_KEY_FONT_SIZE, String(size));
  } else {
    save(STORAGE_KEY_FONT_SIZE, '');
  }
}

function applyFontFamily(): void {
  if (customFontFamily) {
    document.documentElement.style.setProperty('--font-family', customFontFamily);
  } else {
    // Revert to theme default
    const theme = getActiveTheme();
    document.documentElement.style.setProperty('--font-family', theme.fontFamily);
  }
}

function applyFontScale(): void {
  // If a custom pixel size is set, use it directly instead of the scale map
  if (customFontSize !== null) {
    document.documentElement.style.setProperty('--font-size-base', `${customFontSize}px`);
    document.documentElement.style.removeProperty('zoom');
    return;
  }

  const sizeMap: Record<number, string> = {
    0.8: '12px', 0.9: '13px', 1.0: '14px', 1.1: '15px',
    1.2: '16px', 1.3: '17px', 1.4: '18px', 1.5: '19px',
  };
  const snapped = Math.round(fontScale * 10) / 10;
  const size = sizeMap[snapped] || `${Math.round(14 * fontScale)}px`;
  document.documentElement.style.setProperty('--font-size-base', size);
  document.documentElement.style.removeProperty('zoom');
}

let transitionTimer: ReturnType<typeof setTimeout> | null = null;

export function applyTheme(animate = true): void {
  const theme = getActiveTheme();
  const root = document.documentElement;

  // Enable transition class for smooth color fade
  if (animate && root.style.getPropertyValue('--canvas-bg')) {
    root.classList.add('theme-transitioning');
    if (transitionTimer) clearTimeout(transitionTimer);
    transitionTimer = setTimeout(() => {
      root.classList.remove('theme-transitioning');
      transitionTimer = null;
    }, 350);
  }

  root.setAttribute('data-theme', activeThemeId);
  root.style.setProperty('--canvas-bg', theme.canvasBg);
  root.style.setProperty('--card-bg', theme.cardBg);
  root.style.setProperty('--panel-bg', theme.panelBg);
  root.style.setProperty('--surface-border', theme.surfaceBorder);
  root.style.setProperty('--infra-accent', theme.infraAccent);
  root.style.setProperty('--agent-accent', theme.agentAccent);
  root.style.setProperty('--alert-accent', theme.alertAccent);
  root.style.setProperty('--success-accent', theme.successAccent);
  root.style.setProperty('--warning-accent', theme.warningAccent);
  root.style.setProperty('--text-primary', theme.textPrimary);
  root.style.setProperty('--text-secondary', theme.textSecondary);
  root.style.setProperty('--text-muted', theme.textMuted);
  root.style.setProperty('--font-family', theme.fontFamily);
  root.style.setProperty('--mono-family', theme.monoFamily);
  root.style.setProperty('--text-transform', theme.textTransform);
  root.style.setProperty('--border-radius', theme.borderRadius);
  root.style.setProperty('--border-width', theme.borderWidth);

  // Structural properties
  root.style.setProperty('--surface-treatment', theme.surfaceTreatment);
  root.style.setProperty('--card-shadow', theme.cardShadow);
  root.style.setProperty('--surface-opacity', String(theme.surfaceOpacity));
  root.style.setProperty('--backdrop-blur', theme.backdropBlur);
  root.style.setProperty('--border-style', theme.borderStyle);
  root.style.setProperty('--border-opacity', String(theme.borderOpacity));
  root.style.setProperty('--heading-weight', String(theme.headingWeight));
  root.style.setProperty('--body-weight', String(theme.bodyWeight));
  const spacingMap = { tight: '-0.01em', normal: '0', wide: '0.03em' };
  root.style.setProperty('--letter-spacing', spacingMap[theme.letterSpacing]);
  root.style.setProperty('--glow-intensity', String(theme.glowIntensity));
  root.style.setProperty('--glow-color', theme.glowColor);
  const speedMap = { fast: '100ms', normal: '200ms', slow: '350ms' };
  root.style.setProperty('--transition-speed', speedMap[theme.transitionSpeed]);
}

export function initTheme(): void {
  // Restore saved preferences
  const savedTheme = load(STORAGE_KEY_THEME);
  if (savedTheme && THEMES[savedTheme]) {
    activeThemeId = savedTheme;
  }

  const savedFontMode = load(STORAGE_KEY_FONT_MODE);
  if (savedFontMode === 'sans' || savedFontMode === 'serif') {
    fontMode = savedFontMode;
  }

  const savedScale = load(STORAGE_KEY_FONT_SCALE);
  if (savedScale) {
    fontScale = Math.max(0.8, Math.min(1.5, parseFloat(savedScale)));
  }

  const savedFontFamily = load(STORAGE_KEY_FONT_FAMILY);
  if (savedFontFamily) {
    customFontFamily = savedFontFamily;
  }

  const savedFontSize = load(STORAGE_KEY_FONT_SIZE);
  if (savedFontSize) {
    const parsed = parseInt(savedFontSize, 10);
    if (!isNaN(parsed) && parsed >= 8 && parsed <= 32) {
      customFontSize = parsed;
    }
  }

  applyTheme(false);
  applyFontScale();
  applyFontFamily();
  applyCustomColors();
}

/** Apply any saved custom color overrides on top of the active theme. */
function applyCustomColors(): void {
  const saved = load(STORAGE_KEY_CUSTOM_THEME);
  if (!saved) return;
  try {
    const colors = JSON.parse(saved) as Record<string, string>;
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
    for (const [key, value] of Object.entries(colors)) {
      const cssProp = cssMap[key];
      if (cssProp && value) {
        document.documentElement.style.setProperty(cssProp, value);
      }
    }
  } catch {}
}

// --- localStorage helpers ---

function save(key: string, value: string): void {
  try { localStorage.setItem(key, value); } catch {}
}

function load(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}
