<script lang="ts">
  import AttentionToast from './AttentionToast.svelte';
  import {
    getVisibleToasts,
    resolveAttention,
    dismissAttention,
    pushAttention,
  } from '$lib/stores/attention.svelte';
  import type { AttentionItem } from '$lib/stores/attention.svelte';
  import { respondToApproval, respondToHil, acknowledgeAlert, approveA2ACollaboration, extractErrorMessage } from '$lib/tauri/commands';

  let visibleToasts = $derived(getVisibleToasts());

  async function handleResolve(id: string, response: any) {
    const item = visibleToasts.find(i => i.id === id);
    if (!item) return;

    try {
      if (item.type === 'approval' || item.type === 'veto') {
        await respondToApproval(item.agentId, response.toolCallId, response.approved);
      } else if (item.type === 'hil') {
        await respondToHil(item.agentId, response.response, response.selectedOption);
      } else if (item.type === 'a2a_request') {
        if (response.mode === 'messaging' || response.mode === 'messaging_workspace') {
          await approveA2ACollaboration(
            response.sourceAgentId,
            response.targetAgentId,
            response.mode === 'messaging_workspace',
          );
          pushAttention({
            id: `a2a-approved-${response.sourceAgentId}-${response.targetAgentId}-${Date.now()}`,
            agentId: item.agentId,
            agentName: item.agentName,
            agentColor: item.agentColor,
            priority: 'low',
            type: 'completion',
            title: 'A2A ENABLED',
            body: response.mode === 'messaging_workspace'
              ? `Messaging and workspace access enabled between ${item.metadata.sourceAgentName} and ${item.metadata.targetAgentName}.`
              : `Messaging enabled between ${item.metadata.sourceAgentName} and ${item.metadata.targetAgentName}.`,
            metadata: {},
            persistent: false,
            dismissAfterMs: 4000,
            createdAt: Date.now(),
            resolved: false,
          });
        }
      } else if (item.type === 'agent_alert' && item.metadata.blocking) {
        await acknowledgeAlert(item.agentId, item.metadata.alertId);
      }
    } catch (e) {
      const message = extractErrorMessage(e);
      if (
        item.type === 'hil' &&
        message.includes(`No pending HIL request for agent '${item.agentId}'`)
      ) {
        // Another surface, usually the chat tile, already resolved the backend channel.
      } else {
        console.error('Failed to resolve attention item:', e);
        pushAttention({
          id: `attention-resolve-error-${item.id}-${Date.now()}`,
          agentId: item.agentId,
          agentName: item.agentName,
          agentColor: item.agentColor,
          priority: 'high',
          type: 'error',
          title: 'ATTENTION ERROR',
          body: `Failed to resolve ${item.title}: ${message}`,
          metadata: {},
          persistent: false,
          dismissAfterMs: 8000,
          createdAt: Date.now(),
          resolved: false,
        });
        return;
      }
    }

    resolveAttention(id);
  }

  function handleDismiss(id: string) {
    dismissAttention(id);
  }
</script>

{#if visibleToasts.length > 0}
  <div class="toast-stack">
    {#each visibleToasts as item (item.id)}
      <AttentionToast
        {item}
        onresolve={handleResolve}
        ondismiss={handleDismiss}
      />
    {/each}
  </div>
{/if}

<style>
  .toast-stack {
    position: fixed;
    top: 48px;
    right: 16px;
    z-index: 9999;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-height: calc(100vh - 64px);
    overflow-y: auto;
    pointer-events: auto;
    scrollbar-width: thin;
    scrollbar-color: rgba(255, 255, 255, 0.15) transparent;
  }
</style>
