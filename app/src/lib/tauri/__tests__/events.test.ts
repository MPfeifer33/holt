import { describe, it, expect, vi, beforeEach } from 'vitest';

// Re-import both the mock and events module together after each reset
// so they share the same listeners array.
let events: typeof import('../events');
let mock: typeof import('$lib/test-utils/tauri-event-mock');

beforeEach(async () => {
  vi.resetModules();
  mock = await import('$lib/test-utils/tauri-event-mock');
  mock.clearMockListeners();
  events = await import('../events');
});

// ─── 4B-1: Stream Dispatcher Error Isolation ─────────────────────────

describe('stream dispatcher error isolation', () => {
  it('continues dispatching after a callback throws', async () => {
    const received: string[] = [];

    await events.listenAgentStream(() => received.push('first'));
    await events.listenAgentStream(() => { throw new Error('boom'); });
    await events.listenAgentStream(() => received.push('third'));

    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    mock.emitMockEvent('agent-stream', {
      agent_id: 'a1',
      event_type: 'Token',
      data: {},
    });

    expect(received).toEqual(['first', 'third']);
    errorSpy.mockRestore();
  });

  it('logs error with callback index', async () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    await events.listenAgentStream(() => {});
    await events.listenAgentStream(() => { throw new Error('test-error'); });

    mock.emitMockEvent('agent-stream', {
      agent_id: 'a1',
      event_type: 'Token',
      data: {},
    });

    expect(errorSpy).toHaveBeenCalledOnce();
    expect(errorSpy.mock.calls[0][0]).toContain('1'); // callback index
    errorSpy.mockRestore();
  });

  it('handles all callbacks throwing without crashing', async () => {
    await events.listenAgentStream(() => { throw new Error('one'); });
    await events.listenAgentStream(() => { throw new Error('two'); });
    await events.listenAgentStream(() => { throw new Error('three'); });

    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    // Should not throw
    expect(() => {
      mock.emitMockEvent('agent-stream', {
        agent_id: 'a1',
        event_type: 'Token',
        data: {},
      });
    }).not.toThrow();

    expect(errorSpy).toHaveBeenCalledTimes(3);
    errorSpy.mockRestore();
  });
});

// ─── 4B-2: safeCallback wrapper ──────────────────────────────────────

describe('non-stream listener error isolation', () => {
  it('listenHilQuestion catches callback errors', async () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    await events.listenHilQuestion(() => {
      throw new Error('hil-boom');
    });

    expect(() => {
      mock.emitMockEvent('agent-hil-question', {
        agent_id: 'a1',
        question: 'test?',
        options: null,
      });
    }).not.toThrow();

    expect(errorSpy).toHaveBeenCalledOnce();
    expect(errorSpy.mock.calls[0][0]).toContain('agent-hil-question');
    errorSpy.mockRestore();
  });

  it('listenAgentAlert catches callback errors', async () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    await events.listenAgentAlert(() => {
      throw new Error('alert-boom');
    });

    expect(() => {
      mock.emitMockEvent('agent-alert', {
        agent_id: 'a1',
        alert_id: 'x',
        message: 'test',
        priority: 'low',
        blocking: false,
      });
    }).not.toThrow();

    expect(errorSpy).toHaveBeenCalledOnce();
    errorSpy.mockRestore();
  });

  it('passes payload through on success', async () => {
    let received: unknown = null;

    await events.listenHilQuestion((evt) => {
      received = evt;
    });

    mock.emitMockEvent('agent-hil-question', {
      agent_id: 'a1',
      question: 'choose wisely',
      options: ['a', 'b'],
    });

    expect(received).toEqual({
      agent_id: 'a1',
      question: 'choose wisely',
      options: ['a', 'b'],
    });
  });
});
