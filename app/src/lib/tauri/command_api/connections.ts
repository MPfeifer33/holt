import { invoke } from './shared';

export interface ConnectionInfo {
  id: string;
  provider: string;
  endpoint: string;
  key_ref: string;
  status: string;
  is_local: boolean;
}

export async function isClaudeCodeAvailable(): Promise<boolean> {
  try {
    return await invoke<boolean>('is_claude_code_available');
  } catch {
    return false;
  }
}

export function listConnections(): Promise<ConnectionInfo[]> {
  return invoke<ConnectionInfo[]>('list_connections');
}

export function addCloudApi(provider: string, apiKey: string, endpoint: string): Promise<string> {
  return invoke<string>('add_cloud_api', { provider, apiKey, endpoint });
}

export function testConnection(keyRef: string): Promise<boolean> {
  return invoke<boolean>('test_connection', { keyRef });
}

export function removeConnection(keyRef: string): Promise<void> {
  return invoke<void>('remove_connection', { keyRef });
}

export function deleteApiKey(keyRef: string): Promise<void> {
  return invoke<void>('delete_api_key', { keyRef });
}
