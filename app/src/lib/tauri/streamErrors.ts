export function streamErrorMessage(data: unknown): string {
  if (typeof data === 'string') return data;

  if (data && typeof data === 'object') {
    const record = data as Record<string, unknown>;
    const raw = typeof record.message === 'string' ? record.message : record.error;
    if (typeof raw === 'string') return raw;
    if (raw != null) {
      try {
        return JSON.stringify(raw);
      } catch {
        return String(raw);
      }
    }
  }

  return 'Unknown error';
}
