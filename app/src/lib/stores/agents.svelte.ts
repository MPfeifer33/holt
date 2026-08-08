import type { AgentSummary } from '$lib/tauri/commands';

export interface Connection {
  id: string;
  type: 'a2a' | 'subagent';
  sourceId: string;
  targetId: string;
  active: boolean;
}

export interface AgentActivity {
  line: string;
  timestamp: number;
}

export interface CanvasAgent extends AgentSummary {
  x: number;
  y: number;
  recentActivity?: AgentActivity[];
}

export interface SubagentInfo {
  id: string;
  parentId: string;
  name: string;
  status: string;
}

let agents = $state<CanvasAgent[]>([]);
let connections = $state<Connection[]>([]);
let subagents = $state<SubagentInfo[]>([]);

export function getAgents(): CanvasAgent[] { return agents; }
export function getConnections(): Connection[] { return connections; }
export function getSubagents(): SubagentInfo[] { return subagents; }
export function getSubagentsForAgent(parentId: string): SubagentInfo[] {
  return subagents.filter(s => s.parentId === parentId);
}

export function setAgents(a: CanvasAgent[]): void { agents = a; }
export function setConnections(c: Connection[]): void { connections = c; }

export function upsertSubagent(info: SubagentInfo): void {
  const idx = subagents.findIndex(s => s.id === info.id);
  if (idx >= 0) {
    subagents = subagents.map(s => s.id === info.id ? { ...s, ...info } : s);
  } else {
    subagents = [...subagents, info];
  }
}

export function removeSubagent(id: string): void {
  subagents = subagents.filter(s => s.id !== id);
}

/**
 * Update an agent's properties. Short-circuits if no values actually changed,
 * preventing unnecessary array reassignment and re-renders.
 */
export function updateAgent(id: string, updates: Partial<CanvasAgent>): void {
  const idx = agents.findIndex(a => a.id === id);
  if (idx === -1) return;

  const current = agents[idx];

  // Short-circuit: check if any value actually differs
  let changed = false;
  for (const key of Object.keys(updates) as (keyof CanvasAgent)[]) {
    const newVal = updates[key];
    const oldVal = current[key];
    // Shallow equality — sufficient for primitives and status strings.
    // Object values (like Error status) use JSON comparison.
    if (typeof newVal === 'object' && newVal !== null) {
      if (JSON.stringify(newVal) !== JSON.stringify(oldVal)) { changed = true; break; }
    } else if (newVal !== oldVal) {
      changed = true; break;
    }
  }
  if (!changed) return;

  agents = agents.map(a => a.id === id ? { ...a, ...updates } : a);
}

// ── Batched activity updates ───────────────────────────────────────
// Buffer rapid-fire pushActivity calls (e.g. tool spam during coding)
// and flush once per animation frame to avoid churning the agents array.
let activityBuffer: Map<string, { line: string; timestamp: number }[]> = new Map();
let activityRAF: number | null = null;

function flushActivity(): void {
  activityRAF = null;
  if (activityBuffer.size === 0) return;
  const pending = activityBuffer;
  activityBuffer = new Map();

  agents = agents.map(a => {
    const entries = pending.get(a.id);
    if (!entries) return a;
    const prev = a.recentActivity ?? [];
    const next = [...prev, ...entries].slice(-3);
    return { ...a, recentActivity: next };
  });
}

export function pushActivity(id: string, line: string): void {
  const entries = activityBuffer.get(id) ?? [];
  entries.push({ line, timestamp: Date.now() });
  activityBuffer.set(id, entries);
  if (!activityRAF) {
    activityRAF = requestAnimationFrame(flushActivity);
  }
}

// ── Specialized status transitions ──────────────────────────────────

/** Set an agent as Working. No-op if already Working. */
export function markAgentWorking(id: string): void {
  const agent = agents.find(a => a.id === id);
  if (!agent || agent.status === 'Working') return;
  updateAgent(id, { status: 'Working' });
}

/** Set an agent as Idle. No-op if already Idle. */
export function markAgentIdle(id: string): void {
  const agent = agents.find(a => a.id === id);
  if (!agent || agent.status === 'Idle') return;
  updateAgent(id, { status: 'Idle' });
}

// ── Movement with RAF throttling ────────────────────────────────────

let pendingMoves: Map<string, { x: number; y: number }> = new Map();
let moveRafId: number | null = null;

/**
 * Queue an agent position update. Batched via requestAnimationFrame so
 * rapid pointermove events (60+ fps) only trigger one store update per frame.
 */
export function moveAgent(id: string, x: number, y: number): void {
  pendingMoves.set(id, { x, y });
  if (moveRafId === null) {
    moveRafId = requestAnimationFrame(() => {
      for (const [agentId, pos] of pendingMoves) {
        updateAgent(agentId, { x: pos.x, y: pos.y });
      }
      pendingMoves.clear();
      moveRafId = null;
    });
  }
}

export function addConnection(conn: Connection): void {
  connections = [...connections, conn];
}

export function removeAgentFromStore(id: string): void {
  agents = agents.filter(a => a.id !== id);
  // Also remove any connections involving this agent
  connections = connections.filter(c => c.sourceId !== id && c.targetId !== id);
  // Also remove any subagents of this agent
  subagents = subagents.filter(s => s.parentId !== id);
}

export function removeConnection(id: string): void {
  connections = connections.filter(c => c.id !== id);
}

export function setConnectionActive(id: string, active: boolean): void {
  connections = connections.map(c => c.id === id ? { ...c, active } : c);
}
