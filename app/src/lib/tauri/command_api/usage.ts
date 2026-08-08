import { invoke } from './shared';
import type { AgentColor } from './shared';

export interface AgentUsage {
  agent_id: string;
  agent_name: string;
  model_name: string;
  prompt_tokens: number;
  completion_tokens: number;
  current_context_tokens: number;
  context_window_size: number;
  has_pricing: boolean;
  session_start: string | null;
  message_count: number;
  agent_color: AgentColor;
}

export interface DailyUsage {
  date: string;
  prompt_tokens: number;
  completion_tokens: number;
  cost_usd_cents: number;
}

export interface ModelPricing {
  input_per_million: number;
  output_per_million: number;
}

export interface PricingConfig {
  global: Record<string, ModelPricing>;
}

export function getAgentUsage(agentId: string): Promise<AgentUsage> {
  return invoke<AgentUsage>('get_agent_usage', { agentId });
}

export function getAllUsage(): Promise<AgentUsage[]> {
  return invoke<AgentUsage[]>('get_all_usage');
}

export function getDailyUsage(days?: number): Promise<DailyUsage[]> {
  return invoke<DailyUsage[]>('get_daily_usage', { days: days ?? null });
}

export function getPricingConfig(): Promise<PricingConfig> {
  return invoke<PricingConfig>('get_pricing_config');
}
