import { invoke } from './shared';

export interface WatchConfigResponse {
  path: string;
  recursive: boolean;
  glob_filter: string | null;
  events: string[];
}

export interface TriggerJobResponse {
  id: string;
  name: string;
  agent_id: string;
  cron_expression: string | null;
  message: string;
  enabled: boolean;
  created_at: string;
  last_fired: string | null;
  last_skipped: string | null;
  fire_count: number;
  skip_count: number;
  next_fire: string | null;
  job_type: string;
  watch_config: WatchConfigResponse | null;
  internal: boolean;
}

export async function createTrigger(
  name: string,
  agentId: string,
  cronExpression: string,
  message: string,
): Promise<TriggerJobResponse> {
  return invoke<TriggerJobResponse>('create_trigger', { name, agentId, cronExpression, message });
}

export async function listTriggers(): Promise<TriggerJobResponse[]> {
  return invoke<TriggerJobResponse[]>('list_triggers');
}

export async function updateTrigger(
  jobId: string,
  name?: string,
  cronExpression?: string,
  message?: string,
  enabled?: boolean,
): Promise<TriggerJobResponse> {
  return invoke<TriggerJobResponse>('update_trigger', {
    jobId,
    name,
    cronExpression,
    message,
    enabled,
  });
}

export async function deleteTrigger(jobId: string): Promise<void> {
  return invoke<void>('delete_trigger', { jobId });
}

export async function toggleTrigger(jobId: string): Promise<TriggerJobResponse> {
  return invoke<TriggerJobResponse>('toggle_trigger', { jobId });
}

export async function fireTrigger(jobId: string): Promise<void> {
  return invoke<void>('fire_trigger', { jobId });
}
