import { invoke } from './shared';

export interface MemoryTierCounts {
  hot: number;
  warm: number;
  cold: number;
  archive: number;
}

export interface MemoryStats {
  available: boolean;
  backend: string;
  tier_counts: MemoryTierCounts;
  total_tokens: number;
  last_injection_tokens: number;
  budget_max: number;
  pinned_count: number;
}

export interface MemoryEntry {
  id: string;
  content: string;
  tier: string;
  category: string;
  score: number;
  source: string;
  pinned: boolean;
  created_at: string;
  tokens: number;
}

export function getMemoryStats(agentId: string): Promise<MemoryStats> {
  return invoke<MemoryStats>('get_memory_stats', { agentId });
}

export function getMemoryEntries(agentId: string, limit?: number): Promise<MemoryEntry[]> {
  return invoke<MemoryEntry[]>('get_memory_entries', { agentId, limit });
}

export function searchMemories(agentId: string, query: string, limit?: number): Promise<MemoryEntry[]> {
  return invoke<MemoryEntry[]>('search_memories', { agentId, query, limit });
}

export function acknowledgeAlert(agentId: string, alertId: string): Promise<void> {
  return invoke('acknowledge_alert', { agentId, alertId });
}
