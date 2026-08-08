import { THINKING_BUDGETS } from '$lib/constants';

export type SlashCommandGroup = 'general' | 'anthropic' | 'codex';
export type SlashCommandLane = 'all' | 'anthropic' | 'codex';
export type SlashCommandId =
  | 'commands'
  | 'thinking'
  | 'review'
  | 'compact'
  | 'clear'
  | 'orient'
  | 'model'
  | 'effort'
  | 'approval'
  | 'goal'
  | 'fork'
  | 'rollback'
  | 'shell';

export interface SlashCommandDefinition {
  id: SlashCommandId;
  cmd: `/${string}`;
  aliases?: `/${string}`[];
  args: string;
  desc: string;
  group: SlashCommandGroup;
  lane: SlashCommandLane;
  needsArgument?: boolean;
}

export interface SlashCommandAvailability {
  anthropic: boolean;
  codex: boolean;
}

export const SLASH_COMMANDS: SlashCommandDefinition[] = [
  {
    id: 'commands',
    cmd: '/commands',
    aliases: ['/help'],
    args: '',
    desc: 'List the commands available for this agent lane',
    group: 'general',
    lane: 'all',
  },
  {
    id: 'thinking',
    cmd: '/thinking',
    args: '<level>',
    desc: `Set thinking budget (off, ${Object.keys(THINKING_BUDGETS).join(', ')})`,
    group: 'anthropic',
    lane: 'anthropic',
    needsArgument: true,
  },
  {
    id: 'review',
    cmd: '/review',
    args: '[instructions]',
    desc: 'Run Codex-native review against uncommitted workspace changes',
    group: 'codex',
    lane: 'codex',
  },
  {
    id: 'compact',
    cmd: '/compact',
    args: '',
    desc: 'Force conversation compaction now',
    group: 'general',
    lane: 'all',
  },
  {
    id: 'clear',
    cmd: '/clear',
    args: '',
    desc: 'Clear conversation history and create a checkpoint',
    group: 'general',
    lane: 'all',
  },
  {
    id: 'orient',
    cmd: '/orient',
    aliases: ['/wake'],
    args: '',
    desc: 'Trigger cold start orientation (recall memories, read persona, greet)',
    group: 'general',
    lane: 'all',
  },
  {
    id: 'model',
    cmd: '/model',
    args: '<model-name>',
    desc: 'Switch model mid-thread (e.g. gpt-5.5, o3-pro)',
    group: 'codex',
    lane: 'codex',
    needsArgument: true,
  },
  {
    id: 'effort',
    cmd: '/effort',
    args: '<level>',
    desc: 'Change reasoning effort (low, medium, high)',
    group: 'codex',
    lane: 'codex',
    needsArgument: true,
  },
  {
    id: 'approval',
    cmd: '/approval',
    args: '<policy>',
    desc: 'Change approval policy (never, on-failure, on-request, untrusted)',
    group: 'codex',
    lane: 'codex',
    needsArgument: true,
  },
  {
    id: 'goal',
    cmd: '/goal',
    args: '[text]',
    desc: 'Set or clear the thread mission brief / goal',
    group: 'codex',
    lane: 'codex',
  },
  {
    id: 'fork',
    cmd: '/fork',
    args: '',
    desc: 'Fork the current conversation into a new thread',
    group: 'codex',
    lane: 'codex',
  },
  {
    id: 'rollback',
    cmd: '/rollback',
    args: '<n>',
    desc: 'Undo the last N turns from the thread',
    group: 'codex',
    lane: 'codex',
    needsArgument: true,
  },
  {
    id: 'shell',
    cmd: '/shell',
    args: '<command>',
    desc: 'Run a background terminal command in the Codex sandbox',
    group: 'codex',
    lane: 'codex',
    needsArgument: true,
  },
];

export function availableSlashCommands(availability: SlashCommandAvailability): SlashCommandDefinition[] {
  return SLASH_COMMANDS.filter((command) => {
    if (command.lane === 'all') return true;
    if (command.lane === 'anthropic') return availability.anthropic;
    if (command.lane === 'codex') return availability.codex;
    return false;
  });
}

export function filterSlashCommands(
  query: string,
  availability: SlashCommandAvailability,
): SlashCommandDefinition[] {
  const normalized = query.trim().toLowerCase();
  const commands = availableSlashCommands(availability);
  if (!normalized) return commands;

  return commands.filter((command) => {
    if (command.cmd.startsWith(normalized)) return true;
    if (command.aliases?.some((alias) => alias.startsWith(normalized))) return true;
    if (command.desc.toLowerCase().includes(normalized.replace('/', ''))) return true;
    return false;
  });
}

export function findSlashCommand(
  token: string,
  availability: SlashCommandAvailability,
): SlashCommandDefinition | undefined {
  const normalized = token.trim().toLowerCase();
  return availableSlashCommands(availability).find((command) =>
    command.cmd === normalized || command.aliases?.includes(normalized as `/${string}`),
  );
}

export function slashGroupLabel(group: SlashCommandGroup): string {
  switch (group) {
    case 'general':
      return 'General';
    case 'anthropic':
      return 'Anthropic';
    case 'codex':
      return 'Codex';
  }
}
