import { invoke } from './shared';

export type SkillMode = 'inject' | 'reference' | 'auto';
export type DeliveryMode = 'inject' | 'reference';

export interface SkillSummary {
  file_name: string;
  name: string;
  description: string;
  priority: string;
  mode: SkillMode;
  effective_mode: DeliveryMode;
  trigger_count: number;
  max_tokens: number;
  is_directory: boolean;
  file_count: number;
}

export interface SkillDetail {
  file_name: string;
  name: string;
  description: string;
  priority: string;
  mode: SkillMode;
  effective_mode: DeliveryMode;
  triggers: unknown;
  max_tokens: number;
  body: string;
  raw_content: string;
  is_directory: boolean;
  files: string[];
}

export interface ActiveSkillInfo {
  name: string;
  source: string;
}

export interface ImportFile {
  name: string;
  content: string;
  size_bytes: number;
}

export interface ImportPreview {
  skill_name: string;
  description: string;
  is_directory: boolean;
  files: ImportFile[];
  source_url: string;
  commit_sha: string;
  already_exists: boolean;
}

export function listSkills(): Promise<SkillSummary[]> {
  return invoke<SkillSummary[]>('list_skills');
}

export function getSkill(name: string): Promise<SkillDetail> {
  return invoke<SkillDetail>('get_skill', { name });
}

export function createSkill(fileName: string, content: string): Promise<void> {
  return invoke('create_skill', { fileName, content });
}

export function updateSkill(fileName: string, content: string): Promise<void> {
  return invoke('update_skill', { fileName, content });
}

export function deleteSkill(fileName: string): Promise<void> {
  return invoke('delete_skill', { fileName });
}

export function assignSkillToAgent(agentId: string, skillName: string): Promise<void> {
  return invoke('assign_skill_to_agent', { agentId, skillName });
}

export function unassignSkillFromAgent(agentId: string, skillName: string): Promise<void> {
  return invoke('unassign_skill_from_agent', { agentId, skillName });
}

export function getActiveSkills(agentId: string): Promise<ActiveSkillInfo[]> {
  return invoke<ActiveSkillInfo[]>('get_active_skills', { agentId });
}

export function fetchSkillPreview(url: string): Promise<ImportPreview> {
  return invoke<ImportPreview>('fetch_skill_preview', { url });
}

export function installImportedSkill(
  skillName: string,
  files: ImportFile[],
  sourceUrl: string,
  commitSha: string,
  overwrite: boolean,
): Promise<void> {
  return invoke('install_imported_skill', { skillName, files, sourceUrl, commitSha, overwrite });
}
