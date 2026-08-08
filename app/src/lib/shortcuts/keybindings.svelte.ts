/**
 * Keybindings store — syncs with backend config and provides reactive binding state.
 */
import { invoke } from '@tauri-apps/api/core';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface ParsedBinding {
  modifiers: { ctrl: boolean; shift: boolean; alt: boolean; meta: boolean };
  key: string;
  chord?: string; // second key for chord sequences
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/** Action ID -> raw binding string (e.g. "Ctrl+Shift+A") */
let bindings = $state<Record<string, string>>({});

/** Reverse map: normalized binding string -> action ID */
let reverseMap = $state<Record<string, string>>({});

/** Whether bindings have been loaded from backend */
let loaded = $state(false);

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/** Parse a binding string like "Ctrl+Shift+A" or "Ctrl+K 1" into structured form */
export function parseBinding(raw: string): ParsedBinding {
  const parts = raw.split(' ');
  const mainCombo = parts[0];
  const chord = parts[1]; // undefined if no chord

  const keys = mainCombo.split('+');
  const modifiers = { ctrl: false, shift: false, alt: false, meta: false };

  for (let i = 0; i < keys.length - 1; i++) {
    const mod = keys[i].toLowerCase();
    if (mod === 'ctrl') modifiers.ctrl = true;
    else if (mod === 'shift') modifiers.shift = true;
    else if (mod === 'alt') modifiers.alt = true;
    else if (mod === 'super' || mod === 'meta') modifiers.meta = true;
  }

  const key = keys[keys.length - 1];

  return { modifiers, key, chord };
}

/** Normalize a KeyboardEvent into a comparable string */
export function normalizeEvent(e: KeyboardEvent): string {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push('Ctrl');
  if (e.shiftKey) parts.push('Shift');
  if (e.altKey) parts.push('Alt');
  if (e.metaKey) parts.push('Meta');

  // Normalize the key
  let key = e.key;
  if (key === ' ') key = 'Space';
  else if (key === '`') key = '`';
  else if (key.length === 1) key = key.toUpperCase();
  // Function keys stay as-is (F1, F11, etc.)

  parts.push(key);
  return parts.join('+');
}

/** Normalize a binding string to match event normalization */
function normalizeBinding(raw: string): string {
  const parsed = parseBinding(raw);
  const parts: string[] = [];
  if (parsed.modifiers.ctrl) parts.push('Ctrl');
  if (parsed.modifiers.shift) parts.push('Shift');
  if (parsed.modifiers.alt) parts.push('Alt');
  if (parsed.modifiers.meta) parts.push('Meta');

  let key = parsed.key;
  if (key.length === 1) key = key.toUpperCase();
  parts.push(key);

  return parts.join('+');
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/** Load keybindings from backend. Call once on app init. */
export async function loadKeybindings(): Promise<void> {
  try {
    const result = await invoke<Record<string, string>>('get_keybindings');
    bindings = result;
    rebuildReverseMap();
    loaded = true;
  } catch (e) {
    console.error('Failed to load keybindings:', e);
    // Use empty — will fall through to no shortcuts
  }
}

/** Update a single keybinding. Persists to config. */
export async function updateKeybinding(actionId: string, binding: string): Promise<void> {
  const conflicts = await invoke<[string, string[]][]>('update_keybinding', {
    actionId,
    binding,
  });
  bindings[actionId] = binding;
  rebuildReverseMap();

  if (conflicts.length > 0) {
    console.warn('Keybinding conflicts detected:', conflicts);
  }
}

/** Reset all keybindings to defaults. */
export async function resetKeybindings(): Promise<void> {
  const result = await invoke<Record<string, string>>('reset_keybindings');
  bindings = result;
  rebuildReverseMap();
}

/** Get the raw binding string for an action (for tooltip display) */
export function getBindingDisplay(actionId: string): string | undefined {
  return bindings[actionId];
}

/** Get all bindings (reactive) */
export function getBindings(): Record<string, string> {
  return bindings;
}

/** Check if keybindings are loaded */
export function isLoaded(): boolean {
  return loaded;
}

/**
 * Match a keyboard event against registered bindings.
 * Returns the action ID if matched, or undefined.
 * Does NOT handle chords — use the handler module for that.
 */
export function matchEvent(e: KeyboardEvent): string | undefined {
  const normalized = normalizeEvent(e);
  return reverseMap[normalized];
}

/**
 * Check if a keyboard event matches a chord prefix.
 * Returns the prefix string if it's the first part of any chord binding.
 */
export function matchChordPrefix(e: KeyboardEvent): string | undefined {
  const normalized = normalizeEvent(e);
  // Check if any binding starts with this combo as a chord prefix
  for (const [actionId, raw] of Object.entries(bindings)) {
    const parsed = parseBinding(raw);
    if (parsed.chord && normalizeBinding(raw) === normalized) {
      return normalized;
    }
  }
  return undefined;
}

/**
 * Complete a chord: given the prefix and the second key, find the action.
 */
export function matchChordComplete(prefix: string, secondKey: string): string | undefined {
  for (const [actionId, raw] of Object.entries(bindings)) {
    const parsed = parseBinding(raw);
    if (parsed.chord && normalizeBinding(raw) === prefix) {
      // Compare second key
      const normalizedSecond = secondKey.length === 1 ? secondKey.toUpperCase() : secondKey;
      const expectedSecond = parsed.chord.length === 1 ? parsed.chord.toUpperCase() : parsed.chord;
      if (normalizedSecond === expectedSecond) {
        return actionId;
      }
    }
  }
  return undefined;
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

function rebuildReverseMap() {
  const map: Record<string, string> = {};
  for (const [actionId, raw] of Object.entries(bindings)) {
    const parsed = parseBinding(raw);
    // Only add non-chord bindings to reverse map (chords handled separately)
    if (!parsed.chord) {
      map[normalizeBinding(raw)] = actionId;
    }
  }
  reverseMap = map;
}
