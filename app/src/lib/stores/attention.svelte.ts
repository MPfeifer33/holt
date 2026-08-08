import type { AgentColor } from '$lib/tauri/commands';
import { playHilAlert } from '$lib/audio/hil-alert';
import { RESOLVED_PRUNE_MS, TITLE_FLASH_INTERVAL_MS } from '$lib/constants';

// --- Types ---

export interface AttentionConfig {
  title_flash_on: string;
  audio_on: string;
}

export interface AttentionItem {
  id: string;
  agentId: string;
  agentName: string;
  agentColor: AgentColor;
  priority: 'critical' | 'high' | 'medium' | 'low';
  type: 'approval' | 'hil' | 'error' | 'veto' | 'completion' | 'a2a_wake' | 'a2a_request' | 'agent_alert';
  title: string;
  body: string;
  metadata: Record<string, any>;
  persistent: boolean;
  dismissAfterMs: number | null;
  createdAt: number;
  resolved: boolean;
}

const DEFAULT_ATTENTION_CONFIG: AttentionConfig = {
  title_flash_on: 'critical',
  audio_on: 'critical',
};

// --- State ---

let items = $state<AttentionItem[]>([]);
let titleFlashInterval: ReturnType<typeof setInterval> | null = null;
let pruneInterval: ReturnType<typeof setInterval> | null = null;
let originalTitle = typeof document !== 'undefined' ? (document.title || 'Holt') : 'Holt';

const MAX_ATTENTION_ITEMS = 200;

// Start periodic pruning to prevent unbounded growth in long sessions
// Guard against HMR stacking duplicate intervals
if (typeof globalThis !== 'undefined') {
  if (pruneInterval) clearInterval(pruneInterval);
  pruneInterval = setInterval(() => pruneResolved(), 60_000);
}

// --- Priority helpers ---

const PRIORITY_RANK: Record<string, number> = { critical: 4, high: 3, medium: 2, low: 1 };

function meetsThreshold(priority: string, threshold: string): boolean {
  if (threshold === 'off') return false;
  return (PRIORITY_RANK[priority] ?? 0) >= (PRIORITY_RANK[threshold] ?? 5);
}

// --- Title flash ---

function startTitleFlash(item: AttentionItem) {
  if (typeof document === 'undefined') return;
  if (document.hasFocus()) return;
  if (titleFlashInterval) return;
  const alertTitle = `(!) ${item.agentName} — ${item.title}`;
  let isAlert = true;
  titleFlashInterval = setInterval(() => {
    document.title = isAlert ? alertTitle : originalTitle;
    isAlert = !isAlert;
  }, TITLE_FLASH_INTERVAL_MS);

  const onFocus = () => {
    stopTitleFlash();
    window.removeEventListener('focus', onFocus);
  };
  window.addEventListener('focus', onFocus);
}

function stopTitleFlash() {
  if (titleFlashInterval) {
    clearInterval(titleFlashInterval);
    titleFlashInterval = null;
    if (typeof document !== 'undefined') {
      document.title = originalTitle;
    }
  }
}

// --- Auto-dismiss timers ---

const dismissTimers = new Map<string, ReturnType<typeof setTimeout>>();

function scheduleDismiss(item: AttentionItem) {
  if (!item.dismissAfterMs) return;
  if (item.persistent) return; // persistent items show countdown but don't auto-dismiss
  const timer = setTimeout(() => {
    dismissAttention(item.id);
  }, item.dismissAfterMs);
  dismissTimers.set(item.id, timer);
}

function cancelDismissTimer(id: string) {
  const timer = dismissTimers.get(id);
  if (timer) {
    clearTimeout(timer);
    dismissTimers.delete(id);
  }
}

// --- Public API ---

export function pushAttention(item: AttentionItem): void {
  // Dedup: same agentId + type → update existing (but never dedup actionable items
  // that carry unique backend channels — each must resolve independently)
  const priorityOrder: Record<string, number> = { critical: 4, high: 3, medium: 2, low: 1 };
  const actionableTypes = ['approval', 'veto', 'hil', 'a2a_request'];
  const dedupeKey = actionableTypes.includes(item.type)
    ? item.id // unique per request — never dedup
    : (item.metadata?.dedupeKey ?? `${item.agentId}:${item.type}`);
  const existing = items.find(
    i => !i.resolved && (i.metadata?.dedupeKey ?? `${i.agentId}:${i.type}`) === dedupeKey,
  );
  if (existing) {
    existing.body = item.body;
    existing.createdAt = item.createdAt;
    existing.metadata = { ...existing.metadata, ...item.metadata };
    // Escalate priority if the new item has a higher one
    if ((priorityOrder[item.priority] ?? 0) > (priorityOrder[existing.priority] ?? 0)) {
      existing.priority = item.priority;
    }
    // Reschedule dismiss timer for the updated item
    cancelDismissTimer(existing.id);
    scheduleDismiss(existing);
    items = items;
    return;
  }

  items = [item, ...items];
  scheduleDismiss(item);

  // Hard cap: evict oldest resolved items if over limit
  if (items.length > MAX_ATTENTION_ITEMS) {
    items = items.filter(i => !i.resolved).concat(
      items.filter(i => i.resolved).slice(0, MAX_ATTENTION_ITEMS / 2)
    );
  }

  const config: AttentionConfig = item.metadata.attentionConfig ?? DEFAULT_ATTENTION_CONFIG;

  if (meetsThreshold(item.priority, config.audio_on)) {
    playHilAlert();
  }

  if (meetsThreshold(item.priority, config.title_flash_on)) {
    startTitleFlash(item);
  }
}

export function resolveAttention(id: string): void {
  const item = items.find(i => i.id === id);
  if (item) {
    item.resolved = true;
    items = items;
    cancelDismissTimer(id);
    if (!hasPendingFlashWorthy()) stopTitleFlash();
    // Prune old resolved items to prevent unbounded growth
    pruneResolved();
  }
}

export function dismissAttention(id: string): void {
  items = items.filter(i => i.id !== id);
  cancelDismissTimer(id);
  if (!hasPendingFlashWorthy()) stopTitleFlash();
}

export function resolveAllForAgent(agentId: string): void {
  for (const item of items) {
    if (item.agentId === agentId && !item.resolved) {
      // Never auto-resolve items with backend channels — those require explicit user action
      if (item.type === 'approval' || item.type === 'veto' || item.type === 'hil' || item.type === 'a2a_request') continue;
      item.resolved = true;
      cancelDismissTimer(item.id);
    }
  }
  items = items;
  if (!hasPendingFlashWorthy()) stopTitleFlash();
}

export function pauseDismiss(id: string): void {
  cancelDismissTimer(id);
}

export function resumeDismiss(item: AttentionItem): void {
  if (item.dismissAfterMs && !item.resolved) {
    scheduleDismiss(item);
  }
}

export function getVisibleToasts(): AttentionItem[] {
  return items.filter(i => !i.resolved);
}

export function getOverflowCount(): number {
  return 0; // No overflow — all items visible in scrollable stack
}

export function getAllUnresolved(): AttentionItem[] {
  return items.filter(i => !i.resolved);
}

export function hasPendingCritical(): boolean {
  return items.some(i => !i.resolved && i.priority === 'critical');
}

/** Check if any unresolved item would trigger a title flash */
function hasPendingFlashWorthy(): boolean {
  return items.some(i => {
    if (i.resolved) return false;
    const config: AttentionConfig = i.metadata.attentionConfig ?? DEFAULT_ATTENTION_CONFIG;
    return meetsThreshold(i.priority, config.title_flash_on);
  });
}

/** Prune resolved items older than 60 seconds to prevent unbounded growth */
export function pruneResolved(): void {
  const cutoff = Date.now() - RESOLVED_PRUNE_MS;
  items = items.filter(i => !i.resolved || i.createdAt > cutoff);
}

export function getHighestAttentionForAgent(agentId: string): AttentionItem | null {
  const agentItems = items
    .filter(i => !i.resolved && i.agentId === agentId)
    .sort((a, b) => (PRIORITY_RANK[b.priority] ?? 0) - (PRIORITY_RANK[a.priority] ?? 0));
  return agentItems[0] ?? null;
}
