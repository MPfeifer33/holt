import { invoke } from './shared';

export interface FilePreview {
  path: string;
  name: string;
  size_bytes: number;
  line_count: number;
  content: string;
  base64_content: string | null;
  media_type: string | null;
  is_binary: boolean;
  is_image: boolean;
}

export function getWorkspaceRoot(): Promise<string> {
  return invoke<string>('get_workspace_root');
}

export function setWorkspaceRoot(path: string): Promise<string[]> {
  return invoke<string[]>('set_workspace_root', { path });
}

export function pickFolder(defaultPath?: string): Promise<string | null> {
  return invoke<string | null>('pick_folder', { defaultPath: defaultPath ?? null });
}

export function listDirectory(path: string, includeHidden: boolean): Promise<import('./a2a').FolderEntry[]> {
  return invoke<import('./a2a').FolderEntry[]>('list_directory', { path, includeHidden });
}

export function readFilePreview(path: string, maxLines?: number): Promise<FilePreview> {
  return invoke<FilePreview>('read_file_preview', { path, maxLines: maxLines ?? null });
}
