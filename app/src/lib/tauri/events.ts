import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface StreamEvent {
    agent_id: string;
    event_type: 'Token' | 'ToolCall' | 'ToolResult' | 'Done' | 'Error' | 'Thinking' | 'ConversationUpdated' | 'SessionMemoryUpdated' | 'SessionMemoryWarning' | 'SessionMemoryError' | 'A2AMessage';
    data: Record<string, unknown>;
}

export interface HilQuestionEvent {
    agent_id: string;
    question: string;
    options: string[] | null;
}

export interface HilApprovalEvent {
    agent_id: string;
    tool_name: string;
    tool_call_id: string;
    tool_description: string;
    arguments_preview: string;
    working_directory: string;
    tier: string;
    tier_reason: string;
    timeout_seconds: number | null;
}

// ── Error-safe callback wrapper ───────────────────────────────────────
// Wraps listener callbacks so a throwing subscriber never kills the
// Tauri IPC listener or prevents other callbacks from firing.

function safeCallback<T>(eventName: string, cb: (payload: T) => void): (event: { payload: T }) => void {
    return (event) => {
        try {
            cb(event.payload);
        } catch (e) {
            console.error(`[${eventName}] Listener callback threw:`, e);
        }
    };
}

// ── Agent stream dispatcher ───────────────────────────────────────────
// Instead of N separate Tauri IPC subscriptions to "agent-stream" (each
// crossing the webview bridge independently), we maintain a single IPC
// listener and fan out to JS callbacks. This means one deserialization
// per event instead of N, and no IPC-level overhead per subscriber.

type StreamCallback = (event: StreamEvent) => void;

let streamCallbacks: StreamCallback[] = [];
let streamUnlisten: UnlistenFn | null = null;
let streamSetupPromise: Promise<void> | null = null;

function ensureStreamListener(): Promise<void> {
    if (streamSetupPromise) return streamSetupPromise;
    streamSetupPromise = listen<StreamEvent>('agent-stream', (event) => {
        const payload = event.payload;
        for (let i = 0; i < streamCallbacks.length; i++) {
            try {
                streamCallbacks[i](payload);
            } catch (e) {
                console.error(`[stream-dispatch] Callback ${i} threw:`, e);
            }
        }
    }).then((unlisten) => {
        streamUnlisten = unlisten;
    });
    return streamSetupPromise;
}

/**
 * Subscribe to agent streaming events. All subscribers share a single
 * Tauri IPC listener — the event is deserialized once and dispatched
 * to all callbacks via pure JS.
 *
 * Returns an unlisten function that removes this callback. The
 * underlying IPC listener stays alive (lightweight when idle).
 */
export function listenAgentStream(
    callback: StreamCallback,
): Promise<UnlistenFn> {
    streamCallbacks.push(callback);
    return ensureStreamListener().then(() => {
        return () => {
            streamCallbacks = streamCallbacks.filter((cb) => cb !== callback);
        };
    });
}

/** Listen for HIL question events (agent needs user input). */
export function listenHilQuestion(
    callback: (event: HilQuestionEvent) => void,
): Promise<UnlistenFn> {
    return listen<HilQuestionEvent>('agent-hil-question', safeCallback('agent-hil-question', callback));
}

/** Listen for HIL approval events (agent needs confirmation for a destructive action). */
export function listenHilApproval(
    callback: (event: HilApprovalEvent) => void,
): Promise<UnlistenFn> {
    return listen<HilApprovalEvent>('agent-hil-approval', safeCallback('agent-hil-approval', callback));
}

export interface HilVetoEvent {
    agent_id: string;
    tool_call_id: string;
    tool_name: string;
    detail: string;
    tool_description: string;
    working_directory: string;
    tier_reason: string;
    timeout_seconds: number;
}

export interface SubagentStatusEvent {
    agent_id: string;
    job_id: string;
    name: string;
    status: string;
    result?: string;
}

export interface A2ARequestEvent {
    source_agent_id: string;
    source_agent_name: string;
    target_agent_id: string;
    target_agent_name: string;
    reason: string;
}

/** Listen for HIL veto events (NotifyUnlessVeto tier — auto-proceeds unless user cancels). */
export function listenHilVeto(
    callback: (event: HilVetoEvent) => void,
): Promise<UnlistenFn> {
    return listen<HilVetoEvent>('agent-hil-veto', safeCallback('agent-hil-veto', callback));
}

/** Listen for subagent status updates. */
export function listenSubagentStatus(
    callback: (event: SubagentStatusEvent) => void,
): Promise<UnlistenFn> {
    return listen<SubagentStatusEvent>('subagent-status', safeCallback('subagent-status', callback));
}

/** Listen for A2A collaboration requests. */
export function listenA2ARequest(
    callback: (event: A2ARequestEvent) => void,
): Promise<UnlistenFn> {
    return listen<A2ARequestEvent>('agent-a2a-request', safeCallback('agent-a2a-request', callback));
}

export interface A2AWakeEvent {
    target_agent_id: string;
    from_agent_id: string;
    from_agent_name: string;
}

/** Listen for A2A wake events — an idle agent received a message and should start a new turn. */
export function listenA2AWake(
    callback: (event: A2AWakeEvent) => void,
): Promise<UnlistenFn> {
    return listen<A2AWakeEvent>('agent-a2a-wake', safeCallback('agent-a2a-wake', callback));
}

export interface AgentAlertEvent {
    agent_id: string;
    alert_id: string;
    message: string;
    priority: string;
    blocking: boolean;
}

export function listenAgentAlert(
    callback: (event: AgentAlertEvent) => void,
): Promise<UnlistenFn> {
    return listen<AgentAlertEvent>('agent-alert', safeCallback('agent-alert', callback));
}
