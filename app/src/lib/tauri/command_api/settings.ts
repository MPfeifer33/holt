import { invoke } from './shared';

export interface UiConfig {
  constellation_physics_enabled: boolean;
  constellation_node_glow: boolean;
  animation_duration_ms: number;
  glass_opacity: number;
  thought_log_visible: boolean;
  tools_panel_collapsed: boolean;
  power_user_mode: boolean;
  gfx_mode: boolean;
}

export interface WebToolsConfig {
  search_provider: string;
  search_endpoint: string;
  fetch_max_bytes: number;
  fetch_rate_limit_per_minute: number;
  search_rate_limit_per_minute: number;
}

export function getUiConfig(): Promise<UiConfig> {
  return invoke<UiConfig>('get_ui_config');
}

export function updateUiSettings(params: {
  glassOpacity?: number;
  animationMs?: number;
  powerUser?: boolean;
  gfxMode?: boolean;
}): Promise<void> {
  return invoke<void>('update_ui_settings', params);
}

export function getAppSettings(): Promise<{ minimize_to_tray: boolean }> {
  return invoke<{ minimize_to_tray: boolean }>('get_app_settings');
}

export function updateSystemSettings(params: {
  minimizeToTray?: boolean;
  offlineMode?: boolean;
  autoDetect?: boolean;
  detectInterval?: number;
  sessionRestore?: boolean;
  autoSaveInterval?: number;
}): Promise<void> {
  return invoke<void>('update_system_settings', params);
}

export function updateTraceSettings(params: {
  enabled?: boolean;
  retentionDays?: number;
  maxDbMb?: number;
}): Promise<void> {
  return invoke<void>('update_trace_settings', params);
}

export function getSearchConfig(): Promise<WebToolsConfig> {
  return invoke<WebToolsConfig>('get_search_config');
}

export function updateSearchConfig(params: {
  provider?: string;
  endpoint?: string;
}): Promise<void> {
  return invoke<void>('update_search_config', params);
}

export function updateFetchSettings(params: {
  maxBytes?: number;
  rateLimit?: number;
  searchRateLimit?: number;
}): Promise<void> {
  return invoke<void>('update_fetch_settings', params);
}

export function updateAnthropicSettings(
  agentId: string,
  model?: string,
  thinkingEnabled?: boolean,
  thinkingBudget?: number,
): Promise<void> {
  return invoke('update_anthropic_settings', {
    agentId,
    model: model ?? null,
    thinkingEnabled: thinkingEnabled ?? null,
    thinkingBudget: thinkingBudget ?? null,
  });
}
