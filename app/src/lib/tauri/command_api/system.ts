import { invoke } from './shared';

export interface AppMetadata {
  name: string;
  version: string;
}

export interface SystemPressure {
  cpu_percent: number;
  available_memory_mb: number;
  total_memory_mb: number;
}

export function getAppMetadata(): Promise<AppMetadata> {
  return invoke<AppMetadata>('get_app_metadata');
}

export function getSystemPressure(): Promise<SystemPressure> {
  return invoke<SystemPressure>('get_system_pressure');
}
