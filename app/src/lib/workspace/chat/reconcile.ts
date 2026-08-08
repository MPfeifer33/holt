export interface ReconcileMessage {
  timestamp: number;
  localOnly?: boolean;
  localKind?: string;
  resolved?: boolean;
}

export function reconcileConversationMessages<T extends ReconcileMessage>(
  backendMessages: T[],
  currentMessages: T[],
): T[] {
  const preservedLocal = currentMessages.filter((message) =>
    message.localOnly === true && message.resolved !== true
  );

  if (preservedLocal.length === 0) {
    return backendMessages;
  }

  return [...backendMessages, ...preservedLocal]
    .sort((a, b) => a.timestamp - b.timestamp);
}

export function resolveLocalMessages<T extends ReconcileMessage>(
  messages: T[],
  localKind: string,
): T[] {
  let changed = false;
  const next = messages.filter((message) => {
    if (message.localOnly === true && message.localKind === localKind) {
      changed = true;
      return false;
    }
    return true;
  });

  return changed ? next : messages;
}
