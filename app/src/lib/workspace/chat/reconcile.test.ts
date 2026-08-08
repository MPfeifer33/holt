import { describe, expect, it } from 'vitest';
import {
  reconcileConversationMessages,
  resolveLocalMessages,
  type ReconcileMessage,
} from './reconcile';

interface TestMessage extends ReconcileMessage {
  content: string;
}

describe('chat conversation reconciliation', () => {
  it('preserves unresolved local-only messages when backend conversation is replaced', () => {
    const backend: TestMessage[] = [
      { timestamp: 100, content: 'user message' },
      { timestamp: 300, content: 'backend reply' },
    ];
    const current: TestMessage[] = [
      { timestamp: 100, content: 'old user message' },
      {
        timestamp: 200,
        content: 'Failed to send: provider rejected request',
        localOnly: true,
        localKind: 'turn_error',
      },
    ];

    expect(reconcileConversationMessages(backend, current)).toEqual([
      { timestamp: 100, content: 'user message' },
      {
        timestamp: 200,
        content: 'Failed to send: provider rejected request',
        localOnly: true,
        localKind: 'turn_error',
      },
      { timestamp: 300, content: 'backend reply' },
    ]);
  });

  it('does not preserve resolved local-only messages', () => {
    const backend: TestMessage[] = [{ timestamp: 100, content: 'backend' }];
    const current: TestMessage[] = [
      {
        timestamp: 200,
        content: 'old failure',
        localOnly: true,
        localKind: 'turn_error',
        resolved: true,
      },
    ];

    expect(reconcileConversationMessages(backend, current)).toEqual(backend);
  });

  it('removes local messages of a specific kind after a later successful turn', () => {
    const messages: TestMessage[] = [
      { timestamp: 100, content: 'backend' },
      {
        timestamp: 200,
        content: 'Failed to send',
        localOnly: true,
        localKind: 'turn_error',
      },
      {
        timestamp: 300,
        content: 'local status',
        localOnly: true,
        localKind: 'local_status',
      },
    ];

    expect(resolveLocalMessages(messages, 'turn_error')).toEqual([
      { timestamp: 100, content: 'backend' },
      {
        timestamp: 300,
        content: 'local status',
        localOnly: true,
        localKind: 'local_status',
      },
    ]);
  });
});
