import { invoke } from './shared';

export interface TraceEntry {
  id: string;
  timestamp: string;
  agent_id: string;
  trace_type: string;
  content: string;
  outcome: string | null;
}

export function queryRecentTraces(limit: number, agentId?: string): Promise<TraceEntry[]> {
  return invoke<TraceEntry[]>('query_recent_traces', { limit, agentId: agentId ?? null });
}

export function exportTraces(): Promise<string> {
  return invoke<string>('export_traces');
}

export function clearTraces(): Promise<void> {
  return invoke<void>('clear_traces');
}
