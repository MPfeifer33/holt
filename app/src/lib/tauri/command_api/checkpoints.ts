import { invoke } from './shared';

export interface CheckpointSummary {
  id: string;
  label: string;
  created_at: string;
  context_token_estimate: number;
  message_count: number;
}

export function createCheckpoint(agentId: string, label: string): Promise<string> {
  return invoke<string>('create_checkpoint', { agentId, label });
}

export function listCheckpoints(agentId: string): Promise<CheckpointSummary[]> {
  return invoke<CheckpointSummary[]>('list_checkpoints', { agentId });
}

export function restoreCheckpoint(agentId: string, checkpointId: string): Promise<string> {
  return invoke<string>('restore_checkpoint', { agentId, checkpointId });
}

export function deleteCheckpoint(agentId: string, checkpointId: string): Promise<void> {
  return invoke<void>('delete_checkpoint', { agentId, checkpointId });
}
