import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import {
  listenA2ARequest,
  listenA2AWake,
  listenAgentAlert,
  listenAgentStream,
  listenHilApproval,
  listenHilQuestion,
  listenHilVeto,
  listenSubagentStatus,
} from '$lib/tauri/events';
import {
  DISMISS_ALERT_MS,
  DISMISS_ERROR_MS,
  DISMISS_SUBAGENT_MS,
} from '$lib/constants';
import { hilAttentionId } from '$lib/stores/attentionIds';
import { streamErrorMessage } from '$lib/tauri/streamErrors';

type AgentLike = {
  id: string;
  name: string;
  color: { r: number; g: number; b: number };
};

type AttentionPriority = 'critical' | 'high' | 'medium' | 'low';
type AttentionType =
  | 'approval'
  | 'hil'
  | 'error'
  | 'veto'
  | 'completion'
  | 'a2a_wake'
  | 'a2a_request'
  | 'agent_alert';

interface AttentionItemInput {
  id: string;
  agentId: string;
  agentName: string;
  agentColor: { r: number; g: number; b: number };
  priority: AttentionPriority;
  type: AttentionType;
  title: string;
  body: string;
  metadata: Record<string, unknown>;
  persistent: boolean;
  dismissAfterMs: number | null;
  createdAt: number;
  resolved: boolean;
}

interface PageEventListenerDeps {
  getAgents: () => AgentLike[];
  updateAgent: (agentId: string, patch: Record<string, unknown>) => void;
  markAgentWorking: (agentId: string) => void;
  markAgentIdle: (agentId: string) => void;
  pushActivity: (agentId: string, text: string) => void;
  pushAttention: (item: AttentionItemInput) => void;
  resolveAllForAgent: (agentId: string) => void;
  autoPanToAgent: (agentId: string) => void;
  triggerAgentTurn: (agentId: string) => Promise<void>;
  upsertSubagent: (subagent: { id: string; parentId: string; name: string; status: string }) => void;
  removeSubagent: (jobId: string) => void;
  subagentDismissTimeouts: Map<string, ReturnType<typeof setTimeout>>;
  refreshAppearance: (agentId: string) => Promise<void>;
}

const DEFAULT_AGENT_COLOR = { r: 6, g: 182, b: 212 };

function findAgent(agentId: string, getAgents: () => AgentLike[]): AgentLike | undefined {
  return getAgents().find(a => a.id === agentId);
}

export async function setupPageEventListeners({
  getAgents,
  updateAgent,
  markAgentWorking,
  markAgentIdle,
  pushActivity,
  pushAttention,
  resolveAllForAgent,
  autoPanToAgent,
  triggerAgentTurn,
  upsertSubagent,
  removeSubagent,
  subagentDismissTimeouts,
  refreshAppearance,
}: PageEventListenerDeps): Promise<UnlistenFn[]> {
  const unlisteners: UnlistenFn[] = [];

  const unlistenStream = await listenAgentStream((event) => {
    switch (event.event_type) {
      case 'Token':
        markAgentWorking(event.agent_id);
        break;
      case 'ToolCall':
        markAgentWorking(event.agent_id);
        pushActivity(event.agent_id, `tool: ${(event.data.tool as string) ?? 'unknown'}`);
        break;
      case 'A2AMessage':
        pushActivity(event.agent_id, `a2a: ${(event.data.from_name as string) ?? 'another agent'}`);
        break;
      case 'Done': {
        markAgentIdle(event.agent_id);
        pushActivity(event.agent_id, 'completed');
        resolveAllForAgent(event.agent_id);
        break;
      }
      case 'Error': {
        const errorMessage = streamErrorMessage(event.data);
        pushActivity(
          event.agent_id,
          `error: ${errorMessage.slice(0, 60)}`,
        );
        updateAgent(event.agent_id, {
          status: { Error: errorMessage },
        });
        const errAgent = findAgent(event.agent_id, getAgents);
        pushAttention({
          id: `error-${event.agent_id}-${Date.now()}`,
          agentId: event.agent_id,
          agentName: errAgent?.name ?? event.agent_id,
          agentColor: errAgent?.color ?? DEFAULT_AGENT_COLOR,
          priority: 'high',
          type: 'error',
          title: 'ERROR',
          body: errorMessage,
          metadata: {},
          persistent: false,
          dismissAfterMs: DISMISS_ERROR_MS,
          createdAt: Date.now(),
          resolved: false,
        });
        autoPanToAgent(event.agent_id);
        break;
      }
      case 'SessionMemoryUpdated': {
        pushActivity(event.agent_id, 'session memory updated');
        const memoryAgent = findAgent(event.agent_id, getAgents);
        pushAttention({
          id: `session-memory-${event.agent_id}-${Date.now()}`,
          agentId: event.agent_id,
          agentName: memoryAgent?.name ?? event.agent_id,
          agentColor: memoryAgent?.color ?? DEFAULT_AGENT_COLOR,
          priority: 'low',
          type: 'agent_alert',
          title: 'SESSION MEMORY',
          body: 'Session memory updated',
          metadata: { noticeKind: 'session_memory' },
          persistent: false,
          dismissAfterMs: DISMISS_ALERT_MS,
          createdAt: Date.now(),
          resolved: false,
        });
        break;
      }
    }
  });
  unlisteners.push(unlistenStream);

  const unlistenHil = await listenHilQuestion((event) => {
    pushActivity(event.agent_id, 'waiting for input');
    updateAgent(event.agent_id, { status: 'WaitingForHil' });
    const agent = findAgent(event.agent_id, getAgents);
    const attentionId = hilAttentionId(event.agent_id);
    pushAttention({
      id: attentionId,
      agentId: event.agent_id,
      agentName: agent?.name ?? event.agent_id,
      agentColor: agent?.color ?? DEFAULT_AGENT_COLOR,
      priority: 'critical',
      type: 'hil',
      title: 'INPUT NEEDED',
      body: event.question,
      metadata: { options: event.options ?? [], dedupeKey: attentionId },
      persistent: true,
      dismissAfterMs: null,
      createdAt: Date.now(),
      resolved: false,
    });
    autoPanToAgent(event.agent_id);
  });
  unlisteners.push(unlistenHil);

  const unlistenApproval = await listenHilApproval((event) => {
    pushActivity(event.agent_id, `approve: ${event.tool_name}`);
    updateAgent(event.agent_id, { status: 'WaitingForHil' });
    const agent = findAgent(event.agent_id, getAgents);
    pushAttention({
      id: event.tool_call_id,
      agentId: event.agent_id,
      agentName: agent?.name ?? event.agent_id,
      agentColor: agent?.color ?? DEFAULT_AGENT_COLOR,
      priority: 'critical',
      type: 'approval',
      title: 'APPROVAL NEEDED',
      body: `Tool: ${event.tool_name}\n${event.arguments_preview}\n${event.working_directory}`,
      metadata: { toolCallId: event.tool_call_id, toolName: event.tool_name },
      persistent: true,
      dismissAfterMs: null,
      createdAt: Date.now(),
      resolved: false,
    });
    autoPanToAgent(event.agent_id);
  });
  unlisteners.push(unlistenApproval);

  const unlistenVeto = await listenHilVeto((event) => {
    updateAgent(event.agent_id, { status: 'WaitingForHil' });
    const agent = findAgent(event.agent_id, getAgents);
    pushAttention({
      id: event.tool_call_id,
      agentId: event.agent_id,
      agentName: agent?.name ?? event.agent_id,
      agentColor: agent?.color ?? DEFAULT_AGENT_COLOR,
      priority: 'medium',
      type: 'veto',
      title: 'AUTO-PROCEEDING',
      body: `Tool: ${event.tool_name}\n${event.detail}`,
      metadata: {
        toolCallId: event.tool_call_id,
        toolName: event.tool_name,
        timeoutSeconds: event.timeout_seconds,
      },
      persistent: true,
      dismissAfterMs: event.timeout_seconds * 1000, // kept for countdown bar rendering
      createdAt: Date.now(),
      resolved: false,
    });
  });
  unlisteners.push(unlistenVeto);

  const unlistenSubagent = await listenSubagentStatus((event) => {
    const terminalStatuses = ['done', 'complete', 'error', 'cancelled', 'Idle'];
    if (terminalStatuses.includes(event.status)) {
      upsertSubagent({
        id: event.job_id,
        parentId: event.agent_id,
        name: event.name,
        status: event.status,
      });
      const existingTimeout = subagentDismissTimeouts.get(event.job_id);
      if (existingTimeout) clearTimeout(existingTimeout);
      const timeout = setTimeout(() => {
        removeSubagent(event.job_id);
        subagentDismissTimeouts.delete(event.job_id);
      }, DISMISS_SUBAGENT_MS);
      subagentDismissTimeouts.set(event.job_id, timeout);
    } else {
      const existingTimeout = subagentDismissTimeouts.get(event.job_id);
      if (existingTimeout) {
        clearTimeout(existingTimeout);
        subagentDismissTimeouts.delete(event.job_id);
      }
      upsertSubagent({
        id: event.job_id,
        parentId: event.agent_id,
        name: event.name,
        status: event.status,
      });
    }
  });
  unlisteners.push(unlistenSubagent);

  const unlistenA2ARequest = await listenA2ARequest((event) => {
    const sourceAgent = findAgent(event.source_agent_id, getAgents);
    const [left, right] = [event.source_agent_id, event.target_agent_id].sort();
    pushAttention({
      id: `a2a-request-${left}-${right}`,
      agentId: event.source_agent_id,
      agentName: sourceAgent?.name ?? event.source_agent_name,
      agentColor: sourceAgent?.color ?? DEFAULT_AGENT_COLOR,
      priority: 'critical',
      type: 'a2a_request',
      title: 'COLLABORATION REQUEST',
      body: event.reason,
      metadata: {
        dedupeKey: `a2a-request:${left}<->${right}`,
        sourceAgentId: event.source_agent_id,
        sourceAgentName: event.source_agent_name,
        targetAgentId: event.target_agent_id,
        targetAgentName: event.target_agent_name,
        reason: event.reason,
      },
      persistent: true,
      dismissAfterMs: null,
      createdAt: Date.now(),
      resolved: false,
    });
  });
  unlisteners.push(unlistenA2ARequest);

  const unlistenA2AWake = await listenA2AWake(async (event) => {
    const targetAgent = findAgent(event.target_agent_id, getAgents);
    pushAttention({
      id: `a2a-wake-${event.target_agent_id}-${event.from_agent_id}-${Date.now()}`,
      agentId: event.target_agent_id,
      agentName: targetAgent?.name ?? event.target_agent_id,
      agentColor: targetAgent?.color ?? DEFAULT_AGENT_COLOR,
      priority: 'medium',
      type: 'a2a_wake',
      title: 'A2A MESSAGE',
      body: `${event.from_agent_name} sent a direct message.`,
      metadata: {
        dedupeKey: `a2a-wake:${event.target_agent_id}<-${event.from_agent_id}`,
        fromAgentId: event.from_agent_id,
        fromAgentName: event.from_agent_name,
      },
      persistent: false,
      dismissAfterMs: DISMISS_ALERT_MS,
      createdAt: Date.now(),
      resolved: false,
    });
    try {
      await triggerAgentTurn(event.target_agent_id);
    } catch (e) {
      console.error('Failed to trigger agent turn for A2A wake:', e);
    }
  });
  unlisteners.push(unlistenA2AWake);

  const unlistenAlert = await listenAgentAlert((event) => {
    const agent = findAgent(event.agent_id, getAgents);
    pushAttention({
      id: event.alert_id,
      agentId: event.agent_id,
      agentName: agent?.name ?? event.agent_id,
      agentColor: agent?.color ?? DEFAULT_AGENT_COLOR,
      priority: event.priority as AttentionPriority,
      type: 'agent_alert',
      title: 'AGENT ALERT',
      body: event.message,
      metadata: { alertId: event.alert_id, blocking: event.blocking },
      persistent: event.blocking,
      dismissAfterMs: event.blocking ? null : DISMISS_ALERT_MS,
      createdAt: Date.now(),
      resolved: false,
    });
    if (event.priority === 'critical' || event.priority === 'high') {
      autoPanToAgent(event.agent_id);
    }
  });
  unlisteners.push(unlistenAlert);

  const unlistenAppearance = await listen<{ agent_id: string }>(
    'agent-appearance-updated',
    async (event) => {
      await refreshAppearance(event.payload.agent_id);
    },
  );
  unlisteners.push(unlistenAppearance);

  return unlisteners;
}
