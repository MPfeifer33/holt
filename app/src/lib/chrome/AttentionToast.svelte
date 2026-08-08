<script lang="ts">
  import { onMount } from 'svelte';
  import type { AttentionItem } from '$lib/stores/attention.svelte';
  import { pauseDismiss, resumeDismiss } from '$lib/stores/attention.svelte';

  interface Props {
    item: AttentionItem;
    onresolve: (id: string, response: any) => void;
    ondismiss: (id: string) => void;
  }

  let { item, onresolve, ondismiss }: Props = $props();

  // --- Derived values ---

  let agentColorCss = $derived(
    `rgb(${item.agentColor.r}, ${item.agentColor.g}, ${item.agentColor.b})`
  );

  let borderColor = $derived(
    item.priority === 'critical' ? 'var(--alert-accent, #ef4444)' :
    item.priority === 'high' ? 'var(--warning-accent, #f59e0b)' :
    item.priority === 'medium' ? 'var(--agent-accent, #06b6d4)' :
    'var(--surface-border, #334155)'
  );

  let typeLabel = $derived(
    item.type === 'approval' ? 'APPROVAL NEEDED' :
    item.type === 'veto' ? 'AUTO-PROCEEDING' :
    item.type === 'hil' ? 'INPUT NEEDED' :
    item.type === 'error' ? 'ERROR' :
    item.type === 'completion' ? 'TASK COMPLETE' :
    item.type === 'a2a_request' ? 'COLLABORATION REQUEST' :
    item.type === 'a2a_wake' ? 'A2A MESSAGE' :
    item.metadata?.noticeKind === 'session_memory' ? 'SESSION MEMORY' :
    'AGENT ALERT'
  );

  // Elapsed time since creation
  let elapsedSeconds = $state(0);
  let elapsedTimer: ReturnType<typeof setInterval>;

  onMount(() => {
    elapsedSeconds = Math.floor((Date.now() - item.createdAt) / 1000);
    elapsedTimer = setInterval(() => {
      elapsedSeconds = Math.floor((Date.now() - item.createdAt) / 1000);
    }, 1000);
    return () => clearInterval(elapsedTimer);
  });

  let elapsedDisplay = $derived(
    elapsedSeconds < 60 ? `${elapsedSeconds}s` :
    elapsedSeconds < 3600 ? `${Math.floor(elapsedSeconds / 60)}m` :
    `${Math.floor(elapsedSeconds / 3600)}h`
  );

  // --- Metadata helpers ---

  let toolName = $derived(item.metadata?.toolName ?? item.metadata?.tool_name ?? '');
  let toolArgs = $derived(item.metadata?.toolArgs ?? item.metadata?.args ?? '');
  let workingDir = $derived(item.metadata?.workingDirectory ?? item.metadata?.working_directory ?? '');
  let toolCallId = $derived(item.metadata?.toolCallId ?? item.metadata?.tool_call_id ?? '');
  let hilOptions = $derived<string[]>(item.metadata?.options ?? []);
  let alertId = $derived(item.metadata?.alertId ?? item.metadata?.alert_id ?? '');
  let isBlocking = $derived(item.metadata?.blocking ?? false);
  let hasDismissTimer = $derived(item.dismissAfterMs !== null && item.dismissAfterMs > 0);
  let sourceAgentName = $derived(item.metadata?.sourceAgentName ?? item.metadata?.source_agent_name ?? '');
  let targetAgentName = $derived(item.metadata?.targetAgentName ?? item.metadata?.target_agent_name ?? '');
  let sourceAgentId = $derived(item.metadata?.sourceAgentId ?? item.metadata?.source_agent_id ?? '');
  let targetAgentId = $derived(item.metadata?.targetAgentId ?? item.metadata?.target_agent_id ?? '');
  let collaborationReason = $derived(item.metadata?.reason ?? '');
  let hilResponse = $state('');

  // --- Handlers ---

  function handleApprove() {
    onresolve(item.id, { approved: true, toolCallId });
  }

  function handleDeny() {
    onresolve(item.id, { approved: false, toolCallId });
  }

  function handleHilOption(value: string, selectedOption: number) {
    onresolve(item.id, { response: value, selectedOption });
  }

  function submitHilResponse() {
    const response = hilResponse.trim();
    if (!response) return;
    onresolve(item.id, { response });
  }

  function handleAllowMessaging() {
    onresolve(item.id, {
      mode: 'messaging',
      sourceAgentId,
      targetAgentId,
    });
  }

  function handleAllowMessagingAndWorkspace() {
    onresolve(item.id, {
      mode: 'messaging_workspace',
      sourceAgentId,
      targetAgentId,
    });
  }

  function handleDenyCollaboration() {
    onresolve(item.id, {
      mode: 'deny',
      sourceAgentId,
      targetAgentId,
    });
  }

  function handleAcknowledge() {
    onresolve(item.id, { acknowledged: true, alertId });
  }

  function handleClose() {
    ondismiss(item.id);
  }

  function handleMouseEnter() {
    pauseDismiss(item.id);
  }

  function handleMouseLeave() {
    resumeDismiss(item);
  }
</script>

<div
  class="toast"
  style:border-left-color={borderColor}
  style:--dismiss-duration={hasDismissTimer ? `${item.dismissAfterMs}ms` : '0ms'}
  onmouseenter={handleMouseEnter}
  onmouseleave={handleMouseLeave}
  role="alert"
>
  <!-- Header -->
  <div class="toast-header">
    <div class="agent-info">
      <span class="agent-dot" style:background={agentColorCss}></span>
      <span class="agent-name">{item.agentName}</span>
    </div>
    <div class="header-right">
      <span class="elapsed">{elapsedDisplay}</span>
      {#if item.type !== 'approval' && item.type !== 'veto' && item.type !== 'hil' && item.type !== 'a2a_request'}
        <button class="close-btn" onclick={handleClose} aria-label="Dismiss">&#10005;</button>
      {/if}
    </div>
  </div>

  <!-- Type label -->
  <div class="type-label" style:color={borderColor}>{typeLabel}</div>

  <!-- Body: adaptive per type -->
  <div class="toast-body">
    {#if item.type === 'approval'}
      <div class="context-line">
        <span class="label">Tool:</span>
        <span class="tool-name">{toolName}</span>
      </div>
      {#if toolArgs}
        <div class="command-line">{toolArgs}</div>
      {/if}
      {#if workingDir}
        <div class="working-dir">{workingDir}</div>
      {/if}

    {:else if item.type === 'veto'}
      <div class="context-line">
        <span class="label">Tool:</span>
        <span class="tool-name">{toolName}</span>
      </div>
      {#if item.body}
        <div class="body-text">{item.body}</div>
      {/if}

    {:else if item.type === 'hil'}
      <div class="body-text">{item.body}</div>

    {:else if item.type === 'a2a_request'}
      <div class="body-text">
        {sourceAgentName || item.agentName} wants to collaborate with {targetAgentName || 'another agent'}.
      </div>
      {#if collaborationReason}
        <div class="body-text">{collaborationReason}</div>
      {/if}

    {:else if item.type === 'error'}
      <div class="body-text error-text">{item.body}</div>

    {:else if item.type === 'completion'}
      <div class="body-text">{item.body || `${item.agentName} finished`}</div>

    {:else if item.type === 'a2a_wake'}
      <div class="body-text">{item.body || `Message from ${item.agentName}`}</div>

    {:else if item.type === 'agent_alert'}
      <div class="body-text">{item.body}</div>
    {/if}
  </div>

  <!-- Actions: adaptive per type -->
  <div class="toast-actions">
    {#if item.type === 'approval'}
      <button class="btn btn-approve" onclick={handleApprove}>Approve</button>
      <button class="btn btn-deny" onclick={handleDeny}>Deny</button>

    {:else if item.type === 'veto'}
      <button class="btn btn-approve" onclick={handleApprove}>Allow</button>
      <button class="btn btn-deny" onclick={handleDeny}>Deny</button>

    {:else if item.type === 'hil' && hilOptions.length > 0}
      {#each hilOptions as opt, index}
        <button class="btn btn-option" onclick={() => handleHilOption(opt, index)}>{opt}</button>
      {/each}

    {:else if item.type === 'hil'}
      <form
        class="hil-form"
        onsubmit={(event) => {
          event.preventDefault();
          submitHilResponse();
        }}
      >
        <input
          class="hil-response-input"
          type="text"
          bind:value={hilResponse}
          placeholder="Type your response..."
          aria-label="Response for {item.agentName}"
        />
        <button class="btn btn-option" disabled={!hilResponse.trim()} type="submit">Submit</button>
      </form>

    {:else if item.type === 'a2a_request'}
      <button class="btn btn-option" onclick={handleAllowMessaging}>Allow Messaging</button>
      <button class="btn btn-option" onclick={handleAllowMessagingAndWorkspace}>Allow + Workspace</button>
      <button class="btn btn-deny" onclick={handleDenyCollaboration}>Deny</button>

    {:else if item.type === 'error'}
      <button class="btn btn-dismiss" onclick={handleClose}>Dismiss</button>

    {:else if item.type === 'completion'}
      <button class="btn btn-dismiss" onclick={handleClose}>Dismiss</button>

    {:else if item.type === 'a2a_wake'}
      <button class="btn btn-dismiss" onclick={handleClose}>Dismiss</button>

    {:else if item.type === 'agent_alert' && isBlocking}
      <button class="btn btn-approve" onclick={handleAcknowledge}>Acknowledge</button>

    {:else if item.type === 'agent_alert'}
      <button class="btn btn-dismiss" onclick={handleClose}>Dismiss</button>
    {/if}
  </div>

  <!-- Countdown bar -->
  {#if hasDismissTimer}
    <div class="countdown-track">
      <div class="countdown-bar"></div>
    </div>
  {/if}
</div>

<style>
  .toast {
    width: 420px;
    max-width: 100%;
    background: var(--card-bg, rgba(15, 23, 42, 0.95));
    border: 1px solid var(--surface-border, #334155);
    border-left: 3px solid var(--agent-accent, #06b6d4);
    border-radius: var(--border-radius, 8px);
    overflow: hidden;
    pointer-events: auto;
    box-shadow: 0 4px 24px rgba(0, 0, 0, 0.4);
  }

  .toast-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 10px 4px 10px;
  }

  .agent-info {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .agent-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .agent-name {
    font-size: 0.857rem;
    font-weight: 600;
    color: var(--text-primary, #e2e8f0);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .header-right {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .elapsed {
    font-size: 0.786rem;
    font-family: var(--mono-family, monospace);
    color: var(--text-muted, #64748b);
  }

  .close-btn {
    background: none;
    border: none;
    color: var(--text-muted, #64748b);
    font-size: 0.857rem;
    cursor: pointer;
    padding: 2px 4px;
    border-radius: 3px;
    line-height: 1;
  }

  .close-btn:hover {
    color: var(--text-primary, #e2e8f0);
    background: rgba(255, 255, 255, 0.08);
  }

  .type-label {
    font-size: 0.714rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    padding: 0 10px 6px 10px;
    border-bottom: 1px solid var(--surface-border, #334155);
  }

  .toast-body {
    padding: 8px 10px;
    font-size: 0.857rem;
    color: var(--text-secondary, #cbd5e1);
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .context-line {
    display: flex;
    align-items: baseline;
    gap: 6px;
  }

  .label {
    font-size: 0.786rem;
    color: var(--text-muted, #64748b);
  }

  .tool-name {
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    color: var(--text-primary, #e2e8f0);
    font-weight: 600;
  }

  .command-line {
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    color: var(--text-primary, #e2e8f0);
    background: rgba(0, 0, 0, 0.25);
    padding: 4px 6px;
    border-radius: 3px;
    white-space: pre-wrap;
    word-break: break-all;
    max-height: 80px;
    overflow-y: auto;
  }

  .working-dir {
    font-size: 0.786rem;
    color: var(--text-muted, #64748b);
    font-family: var(--mono-family, monospace);
  }

  .body-text {
    font-size: 0.857rem;
    color: var(--text-secondary, #cbd5e1);
    line-height: 1.4;
    word-break: break-word;
  }

  .error-text {
    color: var(--alert-accent, #ef4444);
  }

  .toast-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px 8px 10px;
    flex-wrap: wrap;
  }

  .btn {
    font-size: 0.786rem;
    font-weight: 600;
    padding: 4px 10px;
    border-radius: 4px;
    border: 1px solid var(--surface-border, #334155);
    background: rgba(255, 255, 255, 0.05);
    color: var(--text-secondary, #cbd5e1);
    cursor: pointer;
    transition: background 120ms, color 120ms;
    white-space: nowrap;
  }

  .btn:hover {
    background: rgba(255, 255, 255, 0.12);
    color: var(--text-primary, #e2e8f0);
  }

  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .btn:disabled:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--text-secondary, #cbd5e1);
  }

  .btn-approve {
    border-color: var(--success-accent, #22c55e);
    color: var(--success-accent, #22c55e);
  }

  .btn-approve:hover {
    background: rgba(34, 197, 94, 0.15);
    color: var(--success-accent, #22c55e);
  }

  .btn-deny {
    border-color: var(--alert-accent, #ef4444);
    color: var(--alert-accent, #ef4444);
  }

  .btn-deny:hover {
    background: rgba(239, 68, 68, 0.15);
    color: var(--alert-accent, #ef4444);
  }

  .btn-dismiss {
    color: var(--text-muted, #64748b);
    border-color: transparent;
  }

  .btn-dismiss:hover {
    color: var(--text-primary, #e2e8f0);
  }

  .btn-option {
    border-color: var(--agent-accent, #06b6d4);
    color: var(--agent-accent, #06b6d4);
  }

  .btn-option:hover {
    background: rgba(6, 182, 212, 0.15);
  }

  .hil-form {
    display: flex;
    gap: 6px;
    width: 100%;
  }

  .hil-response-input {
    flex: 1;
    min-width: 0;
    background: rgba(0, 0, 0, 0.25);
    border: 1px solid var(--surface-border, #334155);
    border-radius: 4px;
    color: var(--text-primary, #e2e8f0);
    font-family: inherit;
    font-size: 0.786rem;
    outline: none;
    padding: 4px 8px;
  }

  .hil-response-input:focus {
    border-color: var(--agent-accent, #06b6d4);
  }

  .countdown-track {
    height: 2px;
    background: rgba(255, 255, 255, 0.05);
  }

  .countdown-bar {
    height: 100%;
    background: var(--agent-accent, #06b6d4);
    animation: drain var(--dismiss-duration) linear forwards;
  }

  .toast:hover .countdown-bar {
    animation-play-state: paused;
  }

  @keyframes drain {
    from { width: 100%; }
    to { width: 0%; }
  }
</style>
