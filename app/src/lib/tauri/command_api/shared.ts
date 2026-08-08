import { invoke } from '@tauri-apps/api/core';

export { invoke };

export function extractErrorMessage(err: unknown): string {
  if (typeof err === 'string') return err;
  if (err && typeof err === 'object') {
    const obj = err as Record<string, unknown>;
    if (typeof obj.message === 'string') return obj.message;
  }
  return String(err);
}

export interface AgentColor {
  r: number;
  g: number;
  b: number;
}

export type AgentStatus =
  | 'Idle'
  | 'Working'
  | 'WaitingForHil'
  | { WaitingForAgent: string }
  | { Error: string };

export type AgentProtocol = 'Mcp' | 'ChatCompletions' | 'AnthropicMessages' | 'Codex' | 'AgentSdk';

export function parseAgentStatus(status: AgentStatus): 'idle' | 'working' | 'attention' | 'error' {
  if (typeof status === 'string') {
    switch (status) {
      case 'Idle': return 'idle';
      case 'Working': return 'working';
      case 'WaitingForHil': return 'attention';
      default: return 'idle';
    }
  }
  if ('WaitingForAgent' in status) return 'working';
  if ('Error' in status) return 'error';
  return 'idle';
}

export function statusText(status: AgentStatus): string {
  if (typeof status === 'string') {
    switch (status) {
      case 'Idle': return 'Idle';
      case 'Working': return 'Working';
      case 'WaitingForHil': return 'Needs attention';
      default: return status;
    }
  }
  if ('WaitingForAgent' in status) return 'Waiting for agent';
  if ('Error' in status) return `Error: ${status.Error}`;
  return 'Unknown';
}
