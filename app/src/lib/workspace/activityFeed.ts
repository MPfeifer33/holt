import {
  listenAgentStream,
  listenHilApproval,
  listenHilQuestion,
  listenHilVeto,
  listenSubagentStatus,
  type StreamEvent,
} from '$lib/tauri/events';
import { getToolPreview } from '$lib/utils/toolPreview';
import { queryRecentTraces, type TraceEntry } from '$lib/tauri/commands';
import {
  AGENT_ID_DISPLAY_LEN,
  DEBOUNCE_MS,
  TRUNC_ARGS,
  TRUNC_MESSAGE,
} from '$lib/constants';
import type { UnlistenFn } from '@tauri-apps/api/event';

export type ActivityEntryType = 'tool' | 'msg' | 'hil' | 'veto' | 'err' | 'status' | 'sub';
export type ActivityEntryResult = 'success' | 'fail' | 'pending';

export interface ActivityEntry {
  id: string;
  timestamp: number;
  agentId: string;
  agentName: string;
  type: ActivityEntryType;
  content: string;
  result?: ActivityEntryResult;
}

export const ACTIVITY_BADGE_LABELS: Record<ActivityEntryType, string> = {
  tool: 'TOOL',
  msg: 'MSG',
  hil: 'HIL',
  veto: 'VETO',
  err: 'ERR',
  status: 'STATUS',
  sub: 'SUB',
};

export function formatActivityTime(ts: number): string {
  const d = new Date(ts);
  return d.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

export function activityResultIcon(r?: ActivityEntryResult): string {
  if (r === 'success') return '\u2713';
  if (r === 'fail') return '\u2717';
  if (r === 'pending') return '\u23F3';
  return '';
}

export function activityResultColor(r?: ActivityEntryResult): string {
  if (r === 'success') return 'var(--success-accent, #22c55e)';
  if (r === 'fail') return 'var(--status-error, #ef4444)';
  if (r === 'pending') return 'var(--alert-accent, #f59e0b)';
  return 'transparent';
}

export function activityBadgeClass(type: ActivityEntryType): string {
  return `badge-${type}`;
}

export function defaultAgentName(agentId: string, agents: Array<{ id: string; name: string }>): string {
  return agents.find(a => a.id === agentId)?.name ?? agentId.slice(0, AGENT_ID_DISPLAY_LEN);
}

export function traceTypeToActivityType(traceType: string): ActivityEntryType {
  if (traceType === 'tool_call' || traceType === 'tool_result') return 'tool';
  if (traceType === 'error') return 'err';
  if (traceType === 'hil_message') return 'hil';
  if (traceType === 'a2a_message') return 'msg';
  if (traceType === 'subagent_spawn' || traceType === 'subagent_result') return 'sub';
  return 'msg';
}

interface SharedActivityFeedOptions {
  acceptsAgent: (agentId: string) => boolean;
  addEntry: (entry: Omit<ActivityEntry, 'id' | 'timestamp' | 'agentName'>) => void;
  onError?: (err: unknown) => void;
}

export async function setupSharedActivityFeed({
  acceptsAgent,
  addEntry,
  onError,
}: SharedActivityFeedOptions): Promise<{
  unlisteners: UnlistenFn[];
  cleanup: () => void;
}> {
  const unlisteners: UnlistenFn[] = [];
  const tokenDebounce = new Map<string, ReturnType<typeof setTimeout>>();
  const tokenBuffers = new Map<string, string>();

  function flushTokenBuffer(agentId: string) {
    const buf = tokenBuffers.get(agentId);
    if (buf && acceptsAgent(agentId)) {
      addEntry({ agentId, type: 'msg', content: buf.slice(0, TRUNC_MESSAGE) });
    }
    tokenBuffers.delete(agentId);
    tokenDebounce.delete(agentId);
  }

  const unStream = await listenAgentStream((event: StreamEvent) => {
    if (!acceptsAgent(event.agent_id)) return;

    switch (event.event_type) {
      case 'Token': {
        const token = String(event.data?.text ?? '');
        const existing = tokenBuffers.get(event.agent_id) ?? '';
        tokenBuffers.set(event.agent_id, existing + token);
        const prev = tokenDebounce.get(event.agent_id);
        if (prev) clearTimeout(prev);
        tokenDebounce.set(
          event.agent_id,
          setTimeout(() => flushTokenBuffer(event.agent_id), DEBOUNCE_MS),
        );
        break;
      }
      case 'ToolCall': {
        const toolName = String(event.data?.tool ?? 'unknown');
        const args = getToolPreview(toolName, event.data?.arguments, TRUNC_ARGS) ?? '';
        addEntry({
          agentId: event.agent_id,
          type: 'tool',
          content: `${toolName} ${args}`.trim(),
          result: 'pending',
        });
        break;
      }
      case 'ToolResult': {
        const toolName = String(event.data?.tool ?? 'unknown');
        const result = getToolPreview(toolName, event.data?.result, TRUNC_ARGS) ?? '';
        const status = String(event.data?.status ?? 'done');
        addEntry({
          agentId: event.agent_id,
          type: 'tool',
          content: `${toolName} ${result}`.trim(),
          result: status === 'failed' ? 'fail' : status === 'partial' ? 'pending' : 'success',
        });
        break;
      }
      case 'Done':
        flushTokenBuffer(event.agent_id);
        addEntry({ agentId: event.agent_id, type: 'status', content: 'completed' });
        break;
      case 'Error': {
        const msg = String(event.data?.message ?? 'Unknown error');
        addEntry({ agentId: event.agent_id, type: 'err', content: msg, result: 'fail' });
        break;
      }
    }
  });
  unlisteners.push(unStream);

  const unHilQ = await listenHilQuestion((event) => {
    if (!acceptsAgent(event.agent_id)) return;
    addEntry({
      agentId: event.agent_id,
      type: 'hil',
      content: event.question.slice(0, TRUNC_MESSAGE),
      result: 'pending',
    });
  });
  unlisteners.push(unHilQ);

  const unHilA = await listenHilApproval((event) => {
    if (!acceptsAgent(event.agent_id)) return;
    addEntry({
      agentId: event.agent_id,
      type: 'hil',
      content: `approval needed: ${event.tool_name}`,
      result: 'pending',
    });
  });
  unlisteners.push(unHilA);

  const unVeto = await listenHilVeto((event) => {
    if (!acceptsAgent(event.agent_id)) return;
    addEntry({
      agentId: event.agent_id,
      type: 'veto',
      content: `${event.tool_name}: ${event.detail}`.slice(0, TRUNC_MESSAGE),
      result: 'pending',
    });
  });
  unlisteners.push(unVeto);

  const unSub = await listenSubagentStatus((event) => {
    if (!acceptsAgent(event.agent_id)) return;
    addEntry({
      agentId: event.agent_id,
      type: 'sub',
      content: `${event.name}: ${event.status}`,
    });
  });
  unlisteners.push(unSub);


  return {
    unlisteners,
    cleanup() {
      for (const timer of tokenDebounce.values()) {
        clearTimeout(timer);
      }
      // Flush any pending token buffers before clearing — prevents data loss
      // if cleanup fires before the agent's Done event
      for (const agentId of tokenBuffers.keys()) {
        flushTokenBuffer(agentId);
      }
      tokenDebounce.clear();
      tokenBuffers.clear();
    },
  };
}

export async function backfillActivityEntries(
  agentId: string,
  getAgentName: (agentId: string) => string,
  limit: number,
): Promise<ActivityEntry[]> {
  const traces = await queryRecentTraces(limit, agentId);
  if (traces.length === 0) return [];

  return traces.reverse().map((t: TraceEntry) => ({
    id: t.id,
    timestamp: new Date(t.timestamp).getTime(),
    agentId: t.agent_id,
    agentName: getAgentName(t.agent_id),
    type: traceTypeToActivityType(t.trace_type),
    content: t.content.slice(0, TRUNC_MESSAGE),
    result:
      t.outcome === 'success'
        ? 'success'
        : t.outcome === 'failure'
          ? 'fail'
          : undefined,
  }));
}
