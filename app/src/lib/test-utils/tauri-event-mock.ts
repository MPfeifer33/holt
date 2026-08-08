/**
 * Mock for @tauri-apps/api/event
 *
 * Provides a fake `listen()` that captures callbacks for manual invocation
 * in tests. No actual IPC — pure JS.
 */

export type UnlistenFn = () => void;

interface EventCallback<T> {
  event: string;
  callback: (event: { payload: T }) => void;
}

const listeners: EventCallback<unknown>[] = [];

/**
 * Mock Tauri listen(). Registers a callback and returns an unlisten function.
 */
export async function listen<T>(
  event: string,
  callback: (event: { payload: T }) => void,
): Promise<UnlistenFn> {
  const entry: EventCallback<unknown> = {
    event,
    callback: callback as (event: { payload: unknown }) => void,
  };
  listeners.push(entry);
  return () => {
    const idx = listeners.indexOf(entry);
    if (idx >= 0) listeners.splice(idx, 1);
  };
}

/**
 * Emit a fake event to all listeners registered for the given event name.
 * Used in tests to simulate Tauri backend events.
 */
export function emitMockEvent<T>(event: string, payload: T): void {
  for (const entry of listeners) {
    if (entry.event === event) {
      entry.callback({ payload });
    }
  }
}

/**
 * Clear all registered listeners. Call in afterEach/beforeEach.
 */
export function clearMockListeners(): void {
  listeners.length = 0;
}

/**
 * Get the number of active listeners for an event (or all events).
 */
export function getMockListenerCount(event?: string): number {
  if (event) return listeners.filter((l) => l.event === event).length;
  return listeners.length;
}
