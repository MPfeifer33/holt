// ── Persistent per-agent activity feed ────────────────────────────────
// Module-level $state survives component mount/unmount cycles, so activity
// entries accumulate even when the ActivityTile tab isn't visible.
// Initialized once from setupPageEventListeners; read by ActivityTile.

import type { ActivityEntry } from '$lib/workspace/activityFeed';
import {
  setupSharedActivityFeed,
  backfillActivityEntries,
} from '$lib/workspace/activityFeed';
import { getAgents } from '$lib/stores/agents.svelte';
import { MAX_ACTIVITY_ENTRIES, AGENT_ID_DISPLAY_LEN } from '$lib/constants';
import { tick } from 'svelte';

const MAX_ENTRIES = MAX_ACTIVITY_ENTRIES;

// Module-level reactive state: persists for app lifetime
let feeds: Record<string, ActivityEntry[]> = $state({});
let initialized = false;
let backfilledAgents = new Set<string>();

// Scroll callbacks registered by mounted ActivityTile components
let scrollCallbacks: Map<string, () => void> = new Map();

function agentName(id: string): string {
  return getAgents().find(a => a.id === id)?.name ?? id.slice(0, AGENT_ID_DISPLAY_LEN);
}

function addEntry(entry: Omit<ActivityEntry, 'id' | 'timestamp' | 'agentName'>) {
  const newEntry: ActivityEntry = {
    ...entry,
    id: `act-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    timestamp: Date.now(),
    agentName: agentName(entry.agentId),
  };
  const prev = feeds[entry.agentId] ?? [];
  feeds[entry.agentId] = [...prev.slice(-MAX_ENTRIES + 1), newEntry];

  // Notify mounted tile to auto-scroll
  const cb = scrollCallbacks.get(entry.agentId);
  if (cb) tick().then(cb);
}

/**
 * Initialize the global activity store. Call once at app startup.
 * Sets up event listeners that feed into module-level state.
 */
export async function initActivityStore(): Promise<void> {
  if (initialized) return;
  initialized = true;

  await setupSharedActivityFeed({
    acceptsAgent: () => true,
    addEntry,
    onError: (err) => console.error('ActivityStore: listener error:', err),
  });
}

/** Get activity entries for a specific agent. Reactive — triggers re-render when entries change. */
export function getActivityEntries(agentId: string): ActivityEntry[] {
  return feeds[agentId] ?? [];
}

/** Backfill historical entries from trace storage. Idempotent per agent. */
export async function ensureBackfill(agentId: string): Promise<void> {
  if (backfilledAgents.has(agentId)) return;
  backfilledAgents.add(agentId);

  try {
    const seeded = await backfillActivityEntries(agentId, agentName, MAX_ENTRIES);
    if (seeded.length > 0) {
      const existing = feeds[agentId] ?? [];
      // Merge: historical first, then live entries, deduped by timestamp proximity
      feeds[agentId] = [...seeded, ...existing].slice(-MAX_ENTRIES);
    }
  } catch (err) {
    console.error('ActivityStore: backfill failed for', agentId, err);
  }
}

/** Register a scroll callback for auto-scroll when entries arrive. */
export function registerScrollCallback(agentId: string, cb: () => void): () => void {
  scrollCallbacks.set(agentId, cb);
  return () => scrollCallbacks.delete(agentId);
}
