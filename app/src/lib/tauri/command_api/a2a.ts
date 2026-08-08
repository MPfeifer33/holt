import { invoke } from './shared';
import type { AgentColor } from './shared';

export interface A2AVisibilityConfig {
  messaging_enabled: boolean;
  workspace_visibility: boolean;
}

export interface A2APairEntry {
  source_id: string;
  target_id: string;
  messaging_enabled: boolean;
  workspace_visibility: boolean;
  source: string;
}

export interface MembershipChanges {
  created: number;
  removed: number;
}

export interface CanvasLayout {
  agents: Array<{ id: string; x: number; y: number }>;
  connections: Array<{ id: string; type: string; sourceId: string; targetId: string }>;
}

export interface FolderEntry {
  name: string;
  path: string;
  is_dir: boolean;
  agent_dots: AgentColor[];
}

export function setA2AVisibility(
  sourceId: string,
  targetId: string,
  messagingEnabled: boolean,
  workspaceVisibility: boolean,
): Promise<void> {
  return invoke<void>('set_a2a_visibility', {
    sourceId,
    targetId,
    messagingEnabled,
    workspaceVisibility,
  });
}

export function approveA2ACollaboration(
  sourceId: string,
  targetId: string,
  workspaceVisibility: boolean,
): Promise<void> {
  return invoke<void>('approve_a2a_collaboration', {
    sourceId,
    targetId,
    workspaceVisibility,
  });
}

export function getA2AVisibility(sourceId: string, targetId: string): Promise<A2AVisibilityConfig> {
  return invoke<A2AVisibilityConfig>('get_a2a_visibility', { sourceId, targetId });
}

export async function saveCanvasLayout(layout: CanvasLayout): Promise<void> {
  await invoke('save_orbital_layout', { layoutJson: JSON.stringify(layout) });
}

export async function loadCanvasLayout(): Promise<CanvasLayout | null> {
  const raw = await invoke<string | null>('load_orbital_layout');
  if (!raw) return null;
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

export function listA2APairs(): Promise<A2APairEntry[]> {
  return invoke<A2APairEntry[]>('list_a2a_pairs');
}

export function updateOrbitalMembership(agentId: string, teammates: string[]): Promise<MembershipChanges> {
  return invoke<MembershipChanges>('update_orbital_membership', { agentId, teammates });
}

export function rebuildAllOrbitalMembership(teams: Record<string, string[]>): Promise<number> {
  return invoke<number>('rebuild_all_orbital_membership', { teams });
}
