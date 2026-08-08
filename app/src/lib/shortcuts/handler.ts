/**
 * Global keyboard shortcut handler.
 *
 * Manages focus zones, chord state, and action dispatch.
 * Replaces hardcoded shortcuts in +page.svelte.
 */
import { getCurrentWindow } from '@tauri-apps/api/window';
import {
  matchEvent,
  matchChordPrefix,
  matchChordComplete,
  normalizeEvent,
} from './keybindings.svelte';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type FocusZone = 'canvas' | 'terminal' | 'chat-input' | 'modal';
export type ActionHandler = () => void;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/** Registered action handlers */
const actions: Record<string, ActionHandler> = {};

/** Current chord state */
let chordPrefix: string | null = null;
let chordTimeout: ReturnType<typeof setTimeout> | null = null;
const CHORD_TIMEOUT_MS = 1500;

/** Chord indicator callback (for UI feedback) */
let onChordStart: ((prefix: string) => void) | null = null;
let onChordEnd: (() => void) | null = null;

/** Focus zone detector */
let getFocusZone: (() => FocusZone) | null = null;

// Keys that should pass through to PTY when terminal is focused
const TERMINAL_PASSTHROUGH = new Set([
  'Ctrl+C', 'Ctrl+D', 'Ctrl+Z', 'Ctrl+W', 'Ctrl+L',
  'Ctrl+A', 'Ctrl+E', 'Ctrl+R', 'Ctrl+U', 'Ctrl+K',
]);

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

/**
 * Initialize the shortcut handler. Call once on app mount.
 */
export function initShortcutHandler(options: {
  focusZoneDetector: () => FocusZone;
  chordStart?: (prefix: string) => void;
  chordEnd?: () => void;
}) {
  getFocusZone = options.focusZoneDetector;
  onChordStart = options.chordStart ?? null;
  onChordEnd = options.chordEnd ?? null;
}

/**
 * Register an action handler.
 */
export function registerAction(actionId: string, handler: ActionHandler) {
  actions[actionId] = handler;
}

/**
 * Register multiple action handlers at once.
 */
export function registerActions(handlers: Record<string, ActionHandler>) {
  Object.assign(actions, handlers);
}

/**
 * Unregister an action handler.
 */
export function unregisterAction(actionId: string) {
  delete actions[actionId];
}

// ---------------------------------------------------------------------------
// Core Handler
// ---------------------------------------------------------------------------

/**
 * Handle a global keydown event. Wire this to svelte:window onkeydown.
 * Returns true if the event was consumed by a shortcut.
 */
export function handleKeydown(e: KeyboardEvent): boolean {
  // Ignore raw modifier-only presses
  if (['Control', 'Shift', 'Alt', 'Meta'].includes(e.key)) {
    return false;
  }

  const zone = getFocusZone?.() ?? 'canvas';

  // Modal zone: only Escape passes through
  if (zone === 'modal') {
    if (e.key === 'Escape') {
      const action = actions['focus.escape'];
      if (action) {
        e.preventDefault();
        action();
        return true;
      }
    }
    return false;
  }

  // Terminal zone: pass through terminal-owned sequences
  if (zone === 'terminal') {
    const normalized = normalizeEvent(e);
    if (TERMINAL_PASSTHROUGH.has(normalized)) {
      return false; // Let PTY handle it
    }
    // Tab/Shift+Tab also pass through to terminal
    if (e.key === 'Tab' && !e.ctrlKey) {
      return false;
    }
  }

  // Chat input zone: don't hijack normal typing
  if (zone === 'chat-input') {
    // Only process if a modifier (ctrl/alt/meta) is held (not just shift)
    if (!e.ctrlKey && !e.altKey && !e.metaKey) {
      return false;
    }
  }

  // Chord mode: waiting for second key
  if (chordPrefix !== null) {
    e.preventDefault();
    clearChordTimeout();

    const secondKey = e.key;
    const actionId = matchChordComplete(chordPrefix, secondKey);
    cancelChord();

    if (actionId && actions[actionId]) {
      actions[actionId]();
      return true;
    }
    return false; // chord didn't match
  }

  // Check for chord prefix (e.g. Ctrl+K starts a chord)
  const prefix = matchChordPrefix(e);
  if (prefix) {
    e.preventDefault();
    startChord(prefix);
    return true;
  }

  // Direct match
  const actionId = matchEvent(e);
  if (actionId && actions[actionId]) {
    e.preventDefault();
    actions[actionId]();
    return true;
  }

  return false;
}

// ---------------------------------------------------------------------------
// Chord Management
// ---------------------------------------------------------------------------

function startChord(prefix: string) {
  chordPrefix = prefix;
  onChordStart?.(prefix);
  chordTimeout = setTimeout(() => {
    cancelChord();
  }, CHORD_TIMEOUT_MS);
}

function cancelChord() {
  chordPrefix = null;
  onChordEnd?.();
  clearChordTimeout();
}

function clearChordTimeout() {
  if (chordTimeout !== null) {
    clearTimeout(chordTimeout);
    chordTimeout = null;
  }
}

// ---------------------------------------------------------------------------
// App-Level Actions (Tauri window management)
// ---------------------------------------------------------------------------

export async function appMinimize() {
  try {
    const window = getCurrentWindow();
    await window.minimize();
  } catch (e) {
    console.error('Failed to minimize window:', e);
  }
}

export async function appToggleFullscreen() {
  try {
    const window = getCurrentWindow();
    const isFullscreen = await window.isFullscreen();
    await window.setFullscreen(!isFullscreen);
  } catch (e) {
    console.error('Failed to toggle fullscreen:', e);
  }
}
