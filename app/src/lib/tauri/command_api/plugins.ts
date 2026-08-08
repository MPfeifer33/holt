import { invoke } from './shared';

export interface PluginInfo {
  id: string;
  name: string;
  version: string;
  status: string;
  health: string;
  health_message: string | null;
  tool_count: number;
  enabled: boolean;
}

export function listPlugins(): Promise<PluginInfo[]> {
  return invoke<PluginInfo[]>('list_plugins');
}

export function enablePlugin(pluginId: string): Promise<void> {
  return invoke<void>('enable_plugin', { pluginId });
}

export function disablePlugin(pluginId: string): Promise<void> {
  return invoke<void>('disable_plugin', { pluginId });
}

export function restartPlugin(pluginId: string): Promise<void> {
  return invoke<void>('restart_plugin', { pluginId });
}
