type JsonRecord = Record<string, unknown>;

function parseJsonLike(value: unknown): unknown {
  if (typeof value !== 'string') return value;
  try {
    return JSON.parse(value);
  } catch {
    return value;
  }
}

function asRecord(value: unknown): JsonRecord | null {
  return value && typeof value === 'object' ? value as JsonRecord : null;
}

function compactWhitespace(text: string): string {
  return text.replace(/\s+/g, ' ').trim();
}

function truncate(text: string, maxLength: number): string {
  return text.length > maxLength ? `${text.slice(0, maxLength)}…` : text;
}

function firstUsefulLine(text: string): string | undefined {
  const line = text
    .split('\n')
    .map((segment) => segment.trim())
    .find(Boolean);
  return line ? compactWhitespace(line) : undefined;
}

function stringifyPreview(value: unknown): string | undefined {
  if (value === null || value === undefined) return undefined;
  if (typeof value === 'string') {
    const compact = compactWhitespace(value);
    return compact || undefined;
  }
  try {
    const compact = compactWhitespace(JSON.stringify(value));
    return compact || undefined;
  } catch {
    return undefined;
  }
}

export function getToolPreview(toolName: string | undefined, payload: unknown, maxLength = 88): string | undefined {
  if (!toolName) return truncate(stringifyPreview(payload) ?? '', maxLength) || undefined;

  const parsed = parseJsonLike(payload);
  const record = asRecord(parsed);

  if (toolName === 'bash' || toolName === 'run_bash' || toolName === 'execute_bash' || toolName === 'command_execution') {
    const command = record?.command ?? record?.cmd;
    if (typeof command === 'string' && command.trim()) {
      return truncate(compactWhitespace(command), maxLength);
    }

    const receipt = asRecord(record?.receipt);
    const summary = receipt?.summary;
    if (typeof summary === 'string' && summary.trim()) {
      return truncate(compactWhitespace(summary), maxLength);
    }

    const output = record?.output ?? record?.stdout ?? record?.error;
    if (typeof output === 'string') {
      const line = firstUsefulLine(output);
      if (line) return truncate(line, maxLength);
    }
  }

  if (toolName === 'file_change') {
    const changes = Array.isArray(record?.changes) ? record?.changes : [];
    const first = changes[0];
    if (typeof first === 'string' && first.trim()) {
      return truncate(compactWhitespace(first), maxLength);
    }
    const changeRecord = asRecord(first);
    const path = changeRecord?.path;
    if (typeof path === 'string' && path.trim()) {
      return truncate(path, maxLength);
    }
  }

  const pathLike = record?.path ?? record?.file_path ?? record?.destination_path ?? record?.draft_path;
  if (typeof pathLike === 'string' && pathLike.trim()) {
    return truncate(pathLike, maxLength);
  }

  const queryLike = record?.query ?? record?.pattern ?? record?.heading;
  if (typeof queryLike === 'string' && queryLike.trim()) {
    return truncate(compactWhitespace(queryLike), maxLength);
  }

  const receipt = asRecord(record?.receipt);
  const summary = receipt?.summary;
  if (typeof summary === 'string' && summary.trim()) {
    return truncate(compactWhitespace(summary), maxLength);
  }

  const output = record?.output ?? record?.stdout ?? record?.error ?? record?.content;
  if (typeof output === 'string') {
    const line = firstUsefulLine(output);
    if (line) return truncate(line, maxLength);
  }

  const fallback = stringifyPreview(parsed);
  return fallback ? truncate(fallback, maxLength) : undefined;
}
