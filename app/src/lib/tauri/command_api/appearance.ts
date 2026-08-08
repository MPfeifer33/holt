import { invoke } from './shared';

export interface PinboardNote {
  text: string;
  color?: string;
}

export interface AgentAppearance {
  accent_color?: string;
  glow_color?: string;
  status_message?: string;
  avatar?: string;
  display_name?: string;
  pinboard_notes: PinboardNote[];
}

export function getAgentAppearance(agentId: string): Promise<AgentAppearance | null> {
  return invoke<AgentAppearance | null>('get_agent_appearance', { agentId });
}

export function updateAgentAppearance(agentId: string, appearance: AgentAppearance): Promise<void> {
  return invoke<void>('update_agent_appearance', { agentId, appearance });
}
