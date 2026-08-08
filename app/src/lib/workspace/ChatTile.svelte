<script lang="ts">
  import { onMount, onDestroy, untrack } from 'svelte';
  import { getConversation, sendMessage, respondToHil, respondToApproval, updateAnthropicSettings, addUserNote, clearUserNotes, interruptAgent, getAgentUsage, extractErrorMessage } from '$lib/tauri/commands';
  import type { ImageAttachment } from '$lib/tauri/commands';
  import { resolveAttention } from '$lib/stores/attention.svelte';
  import { THINKING_BUDGETS, MAX_IMAGE_SIZE } from '$lib/constants';
  import { getAgents } from '$lib/stores/agents.svelte';
  import { Marked } from 'marked';
  import { markedHighlight } from 'marked-highlight';
  import hljs from 'highlight.js';
  import DOMPurify from 'dompurify';
  import type { ConversationMessageSummary } from '$lib/tauri/commands';
  import {
    listenAgentStream,
    listenHilQuestion,
    listenHilApproval,
    listenHilVeto,
  } from '$lib/tauri/events';
  import { getToolPreview } from '$lib/utils/toolPreview';
  import {
    filterSlashCommands,
    findSlashCommand,
    slashGroupLabel,
    type SlashCommandDefinition,
  } from '$lib/workspace/slashCommands';
  import { reconcileConversationMessages, resolveLocalMessages } from '$lib/workspace/chat/reconcile';
  import { hilAttentionId } from '$lib/stores/attentionIds';
  import { streamErrorMessage } from '$lib/tauri/streamErrors';
  import type { StreamEvent, HilQuestionEvent, HilApprovalEvent, HilVetoEvent } from '$lib/tauri/events';
  import { type UnlistenFn } from '@tauri-apps/api/event';

  interface Props {
    agentId: string;
  }

  let { agentId }: Props = $props();

  // --- Types ---

  interface ChatMessage {
    type: 'agent' | 'user' | 'system' | 'action' | 'status' | 'hil' | 'approval' | 'thinking';
    content: string;
    id?: string;
    localOnly?: boolean;
    localKind?: 'turn_error' | 'local_status' | 'hil_feedback';
    /** Pre-rendered HTML for agent messages — avoids re-running markdown +
     *  DOMPurify on every Svelte re-render cycle. Computed once on creation. */
    renderedHtml?: string;
    timestamp: number;
    // For images:
    contentBlocks?: { type: string; data?: string; media_type?: string; text?: string }[];
    // For actions:
    toolName?: string;
    toolCallId?: string;
    toolStatus?: 'running' | 'done' | 'partial' | 'failed';
    toolDuration?: number;
    toolOutput?: string;
    actionPreview?: string;
    // For HIL:
    hilId?: string;
    options?: string[];
    resolved?: boolean;
    // For approval:
    approvalTool?: string;
    approvalToolCallId?: string;
    approvalArgs?: string;
    approvalTimeout?: number;
    statusTone?: 'neutral' | 'control' | 'message';
    // Error feedback (e.g. failed HIL response)
    error?: string;
  }

  // --- State ---

  let messages = $state<ChatMessage[]>([]);
  let inputText = $state('');
  let sending = $state(false);
  // Track agent working status from store (handles SDK agents where sendMessage returns immediately)
  let agentWorking = $derived(getAgents().find(a => a.id === agentId)?.status === 'Working');
  let showStopButton = $derived(sending || agentWorking);
  let scrollContainer: HTMLDivElement | undefined = $state();
  let expandedActions = $state<Set<number>>(new Set());
  let hilInputs = $state<Record<number, string>>({});

  // --- Virtualization ---
  // Only render a window of messages to cap DOM node count.
  // Messages are append-only so global indices stay stable.
  const VISIBLE_WINDOW = 150;
  const LOAD_MORE_CHUNK = 100;
  let visibleStart = $state(0);
  let visibleMessages = $derived.by(() => {
    const start = Math.max(0, visibleStart);
    return messages.slice(start).map((msg, localIdx) => ({
      msg,
      globalIndex: start + localIdx,
    }));
  });
  let hasMoreAbove = $derived(visibleStart > 0);

  function loadMoreMessages() {
    visibleStart = Math.max(0, visibleStart - LOAD_MORE_CHUNK);
  }

  // Advance visible window when new messages arrive (keep showing the tail).
  // Reads visibleStart via untrack() to avoid a read→write dependency loop —
  // this effect should only fire when messages.length changes, not when
  // visibleStart changes (which would cause infinite re-triggering).
  $effect(() => {
    const len = messages.length;
    const cur = untrack(() => visibleStart);
    if (len > VISIBLE_WINDOW) {
      const currentEnd = cur + VISIBLE_WINDOW;
      // Only auto-advance if user hasn't scrolled far up
      if (currentEnd >= len - LOAD_MORE_CHUNK || cur === 0) {
        visibleStart = len - VISIBLE_WINDOW;
      }
    } else {
      visibleStart = 0;
    }
  });
  let streamingContent = $state('');
  let renderedStreamHtml = $state('');
  let renderThrottleId: ReturnType<typeof setTimeout> | null = null;
  let lastRenderLength = 0;
  const RENDER_THROTTLE_MS = 150;
  let currentModel = $state('');
  let localMessages = $state<Array<{text: string, timestamp: number}>>([]);
  let destroyed = false;
  let currentTurnHadStreamError = false;

  const MAX_CHAT_MESSAGES = 2500;
  const MAX_LOCAL_MESSAGES = 100;

  function trimMessages(next: ChatMessage[]): ChatMessage[] {
    return next.length > MAX_CHAT_MESSAGES
      ? next.slice(-MAX_CHAT_MESSAGES)
      : next;
  }

  // ── Event batching ──────────────────────────────────────────────────
  // Tool call/result events arrive in bursts. Without batching, each event
  // triggers a full Svelte reactivity cycle (array copy, re-render, scroll).
  // We buffer mutations and flush them together on the next animation frame,
  // so rapid tool events cause a single DOM update instead of N.

  type PendingMutation =
    | { kind: 'append'; message: ChatMessage }
    | { kind: 'update'; index: number; patch: Partial<ChatMessage> };

  let pendingMutations: PendingMutation[] = [];
  let flushScheduled = false;

  function scheduleMutationFlush() {
    if (flushScheduled) return;
    flushScheduled = true;
    requestAnimationFrame(() => {
      flushScheduled = false;
      if (pendingMutations.length === 0) return;
      const batch = pendingMutations;
      pendingMutations = [];

      // Apply all mutations to a shallow copy so Svelte sees a new reference
      const working = [...messages];
      for (const mut of batch) {
        if (mut.kind === 'append') {
          working.push(mut.message);
        } else if (mut.index < working.length) {
          // In-place update on the copy
          const msg = working[mut.index];
          if (msg) {
            Object.assign(msg, mut.patch);
          }
        }
      }
      messages = trimMessages(working);
    });
  }

  /** Pre-render markdown for agent messages so DOMPurify._initDocument2
   *  runs once per message, not on every Svelte re-render cycle. */
  function prepareMessage(message: ChatMessage): ChatMessage {
    if (message.type === 'agent' && !message.renderedHtml) {
      message.renderedHtml = renderMarkdown(message.content);
    }
    return message;
  }

  function appendMessage(message: ChatMessage): void {
    pendingMutations.push({ kind: 'append', message: prepareMessage(message) });
    scheduleMutationFlush();
  }

  /** Immediate append -- bypasses batching for user-initiated messages
   *  where we want instant feedback (send, HIL responses, etc.) */
  function appendMessageImmediate(message: ChatMessage): void {
    messages = trimMessages([...messages, prepareMessage(message)]);
  }

  function appendTurnErrorMessage(content: string): void {
    appendMessageImmediate({
      type: 'system',
      id: `turn-error-${agentId}-${Date.now()}`,
      localOnly: true,
      localKind: 'turn_error',
      content,
      timestamp: Date.now(),
    });
  }

  function clearLocalTurnErrors(): void {
    messages = resolveLocalMessages(messages, 'turn_error');
  }

  /** Batch an update to an existing action message. Handles both committed
   *  messages and pending appends that haven't flushed yet. */
  function batchUpdateAction(
    index: number,
    patch: Partial<ChatMessage>,
    toolName?: string,
    result?: unknown,
  ): void {
    // Fill in actionPreview if not already set
    const previewPatch = { ...patch };
    if (index < messages.length) {
      // Updating a committed message
      if (!messages[index].actionPreview && toolName) {
        previewPatch.actionPreview = buildActionPreview(toolName, result);
      }
      pendingMutations.push({ kind: 'update', index, patch: previewPatch });
    } else {
      // Updating a pending append that hasn't flushed yet
      const pendingIndex = index - messages.length;
      const mut = pendingMutations[pendingIndex];
      if (mut && mut.kind === 'append') {
        Object.assign(mut.message, previewPatch);
        if (!mut.message.actionPreview && toolName) {
          mut.message.actionPreview = buildActionPreview(toolName, result);
        }
        // No need to schedule -- it's already scheduled from the append
        return;
      }
    }
    scheduleMutationFlush();
  }

  function registerUnlistener(promise: Promise<UnlistenFn>): void {
    void promise.then((fn) => {
      if (destroyed) {
        fn();
        return;
      }
      unlisteners.push(fn);
    });
  }

  function findActionIndex(toolCallId?: string, toolName?: string, onlyRunning = false): number {
    // Check committed messages
    for (let i = messages.length - 1; i >= 0; i--) {
      const msg = messages[i];
      if (msg.type !== 'action') continue;
      if (onlyRunning && msg.toolStatus !== 'running') continue;
      if (toolCallId && msg.toolCallId === toolCallId) return i;
      if (!toolCallId && toolName && msg.toolName === toolName) return i;
    }
    // Also check pending appends (tool call may be buffered but not yet flushed)
    for (let i = pendingMutations.length - 1; i >= 0; i--) {
      const mut = pendingMutations[i];
      if (mut.kind !== 'append' || mut.message.type !== 'action') continue;
      if (onlyRunning && mut.message.toolStatus !== 'running') continue;
      if (toolCallId && mut.message.toolCallId === toolCallId) {
        // Return a synthetic index: messages.length + position in pending
        // The update will be applied when the flush happens
        return messages.length + i;
      }
      if (!toolCallId && toolName && mut.message.toolName === toolName) {
        return messages.length + i;
      }
    }
    return -1;
  }

  function inferToolStatus(toolOutput?: string): 'done' | 'partial' | 'failed' {
    if (!toolOutput) return 'done';
    try {
      const parsed = JSON.parse(toolOutput) as Record<string, unknown>;
      const explicitStatus = typeof parsed.tool_result_status === 'string' ? parsed.tool_result_status : undefined;
      if (explicitStatus === 'partial') return 'partial';
      if (explicitStatus === 'failed') return 'failed';
      if (typeof parsed.error === 'string') return 'failed';
      const receipt = parsed.receipt as Record<string, unknown> | undefined;
      if (receipt && receipt.verification_status === 'verification_failed') return 'partial';
    } catch {
      // Leave non-JSON output on the normal success path.
    }
    return 'done';
  }

  function formatToolDisplayName(name?: string): string {
    if (!name) return 'Tool';
    if (name === 'command_execution') return 'Run command';
    if (name === 'file_change') return 'Apply file change';
    if (name.includes(':')) {
      const [server, tool] = name.split(':', 2);
      return `${tool} @ ${server}`;
    }
    return name.replaceAll('_', ' ');
  }

  function buildActionPreview(toolName?: string, raw?: unknown): string | undefined {
    const preview = getToolPreview(toolName, raw, 88);
    if (!preview) return undefined;
    return toolName?.includes(':') ? truncatePath(preview, 56) : preview;
  }

  // Derive agent protocol for model picker display
  let agentProtocol = $derived(getAgents().find(a => a.id === agentId)?.protocol ?? 'ChatCompletions');
  let isAnthropicAgent = $derived(agentProtocol === 'AnthropicMessages' || agentProtocol === 'AgentSdk');
  let isCodexAgent = $derived(agentProtocol === 'Codex');
  let showModelPicker = $derived(agentProtocol !== 'Mcp' && agentProtocol !== 'AgentSdk');
  let pendingImages = $state<ImageAttachment[]>([]);

  function appendStatusMessage(
    content: string,
    statusTone: 'neutral' | 'control' | 'message' = 'neutral',
  ) {
    appendMessage({
      type: 'status',
      content,
      timestamp: Date.now(),
      statusTone,
    });
  }

  $effect(() => {
    if (!showCommandPalette) {
      selectedSlashIndex = 0;
      return;
    }
    if (filteredCommands.length === 0) {
      selectedSlashIndex = 0;
      return;
    }
    if (selectedSlashIndex >= filteredCommands.length) {
      selectedSlashIndex = filteredCommands.length - 1;
    }
  });

  // Event listener cleanup
  let unlisteners: UnlistenFn[] = [];

  // --- Markdown renderer ---

  const marked = new Marked(
    markedHighlight({
      langPrefix: 'hljs language-',
      highlight(code: string, lang: string) {
        if (lang && hljs.getLanguage(lang)) {
          return hljs.highlight(code, { language: lang }).value;
        }
        return hljs.highlightAuto(code).value;
      },
    })
  );

  marked.setOptions({
    breaks: true,
    gfm: true,
  });

  function renderMarkdown(text: string): string {
    const raw = marked.parse(text);
    if (typeof raw !== 'string') return DOMPurify.sanitize(text);
    return DOMPurify.sanitize(raw);
  }

  /** Throttled markdown render for streaming content. Only re-parses at most
   *  every RENDER_THROTTLE_MS to avoid main-thread starvation from repeated
   *  full markdown parse + DOMPurify on growing text. First token renders
   *  immediately so the bubble appears without delay. */
  function scheduleStreamRender() {
    // First token: render immediately so streaming bubble isn't empty
    if (lastRenderLength === 0 && streamingContent) {
      lastRenderLength = streamingContent.length;
      renderedStreamHtml = renderMarkdown(streamingContent);
      return;
    }
    if (renderThrottleId !== null) return; // already scheduled
    renderThrottleId = setTimeout(() => {
      renderThrottleId = null;
      if (streamingContent && streamingContent.length !== lastRenderLength) {
        lastRenderLength = streamingContent.length;
        renderedStreamHtml = renderMarkdown(streamingContent);
      }
    }, RENDER_THROTTLE_MS);
  }

  /** Immediately render streaming content (used on Done/ToolCall flush). */
  function flushStreamRender() {
    if (renderThrottleId !== null) {
      clearTimeout(renderThrottleId);
      renderThrottleId = null;
    }
    if (streamingContent) {
      lastRenderLength = streamingContent.length;
      renderedStreamHtml = renderMarkdown(streamingContent);
    } else {
      renderedStreamHtml = '';
      lastRenderLength = 0;
    }
  }

  // --- Helpers ---

  function scrollToBottom() {
    if (scrollContainer) {
      // Use requestAnimationFrame to ensure DOM has updated
      requestAnimationFrame(() => {
        if (scrollContainer) {
          scrollContainer.scrollTop = scrollContainer.scrollHeight;
        }
      });
    }
  }

  function toggleAction(index: number) {
    const next = new Set(expandedActions);
    if (next.has(index)) {
      next.delete(index);
    } else {
      next.add(index);
    }
    expandedActions = next;
  }

  function convertConversation(msgs: ConversationMessageSummary[]): ChatMessage[] {
    const result: ChatMessage[] = [];
    for (const msg of msgs) {
      if (msg.role === 'assistant') {
        // Check for tool calls
        if (msg.tool_calls && msg.tool_calls.length > 0) {
          // Add agent text if present
          if (msg.content && msg.content.trim()) {
            result.push(prepareMessage({
              type: 'agent',
              content: msg.content,
              timestamp: new Date(msg.timestamp).getTime(),
            }));
          }
          // Add tool call actions
          for (const tc of msg.tool_calls) {
            result.push({
              type: 'action',
              content: '',
              timestamp: new Date(msg.timestamp).getTime(),
              toolCallId: tc.id,
              toolName: tc.name,
              toolStatus: 'done',
              toolOutput: typeof tc.arguments === 'string' ? tc.arguments : JSON.stringify(tc.arguments),
            });
          }
        } else {
          result.push(prepareMessage({
            type: 'agent',
            content: msg.content,
            timestamp: new Date(msg.timestamp).getTime(),
          }));
        }
      } else if (msg.role === 'user') {
        result.push({
          type: 'user',
          content: msg.content,
          timestamp: new Date(msg.timestamp).getTime(),
          contentBlocks: msg.content_blocks ?? undefined,
        });
      } else if (msg.role === 'system') {
        result.push({
          type: 'system',
          content: msg.content,
          timestamp: new Date(msg.timestamp).getTime(),
        });
      } else if (msg.role === 'tool') {
        // Tool results — attach to the matching action when we have a tool_call_id.
        const actionIndex = msg.tool_call_id
          ? result.findLastIndex(m => m.type === 'action' && m.toolCallId === msg.tool_call_id)
          : result.findLastIndex(m => m.type === 'action');
        if (actionIndex !== -1) {
          const action = result[actionIndex];
          action.toolOutput = msg.content;
          action.toolStatus = inferToolStatus(msg.content);
        }
      }
    }
    return result;
  }

  // --- Local messages and slash commands ---

  function addLocalMessage(text: string) {
    localMessages = [...localMessages, { text, timestamp: Date.now() }].slice(-MAX_LOCAL_MESSAGES);
  }

  function listAvailableSlashCommands(): void {
    const commands = filterSlashCommands('', {
      anthropic: isAnthropicAgent,
      codex: isCodexAgent,
    });
    const lines = commands.map((command) =>
      `${command.cmd}${command.args ? ` ${command.args}` : ''} — ${command.desc}`,
    );
    addLocalMessage(`Available commands:\n${lines.join('\n')}`);
  }

  function applySlashCommandSelection(command: SlashCommandDefinition): void {
    inputText = command.needsArgument ? `${command.cmd} ` : command.cmd;
    showCommandPalette = false;
    selectedSlashIndex = 0;
    autoResize();
    requestAnimationFrame(() => textareaEl?.focus());
  }

  async function handleSlashCommand(text: string): Promise<boolean> {
    const trimmed = text.trim();
    const [token, ...rest] = trimmed.split(/\s+/);
    const command = findSlashCommand(token, {
      anthropic: isAnthropicAgent,
      codex: isCodexAgent,
    });
    const argumentText = rest.join(' ').trim();

    if (!command) {
      addLocalMessage(`Unknown command: ${token}. Use /commands to see what this agent supports.`);
      return true;
    }

    try {
      if (command.id === 'commands') {
        listAvailableSlashCommands();
        return true;
      }

      if (command.id === 'thinking') {
        if (!argumentText) {
          addLocalMessage(`Usage: /thinking off | ${Object.keys(THINKING_BUDGETS).join(' | ')}`);
          return true;
        }
        if (argumentText === 'off') {
          await updateAnthropicSettings(agentId, undefined, false);
          addLocalMessage('Extended thinking disabled');
        } else {
          const budgets = THINKING_BUDGETS;
          const budget = budgets[argumentText];
          if (!budget) {
            addLocalMessage(`Usage: /thinking off | ${Object.keys(THINKING_BUDGETS).join(' | ')}`);
            return true;
          }
          await updateAnthropicSettings(agentId, undefined, true, budget);
          addLocalMessage(`Extended thinking enabled: ${argumentText} budget`);
        }
        return true;
      }

      if (command.id === 'review') {
        const { runCodexReview } = await import('$lib/tauri/commands');
        addLocalMessage('Running Codex review on uncommitted changes...');
        const result = await runCodexReview(agentId, argumentText || undefined);
        addLocalMessage(`Codex review complete:\n${result || '(no findings)'}`);
        return true;
      }

      if (command.id === 'compact') {
        const { compactAgentContext } = await import('$lib/tauri/commands');
        addLocalMessage('Compacting conversation...');
        const result = await compactAgentContext(agentId);
        addLocalMessage(`Compaction complete: ${result}`);
        return true;
      }

      if (command.id === 'clear') {
        const { clearConversation } = await import('$lib/tauri/commands');
        const checkpointId = await clearConversation(agentId);
        messages = [];
        streamingContent = '';
        addLocalMessage(`Conversation cleared (checkpoint: ${checkpointId})`);
        return true;
      }

      if (command.id === 'orient') {
        const orientPrompt = [
          'Run your cold start orientation protocol now.',
          'Recall your recent session summaries and key memories.',
          'Read your persona files for identity context.',
          'Then respond naturally — not with a checklist, but as yourself.',
        ].join(' ');
        addLocalMessage('Triggering cold start orientation...');
        await sendMessage(agentId, orientPrompt);
        return true;
      }

      // --- Codex thread operations ---
      if (command.id === 'model') {
        if (!argumentText) {
          addLocalMessage('Usage: /model <model-name>');
          return true;
        }
        const { codexSwitchModel } = await import('$lib/tauri/commands');
        const result = await codexSwitchModel(agentId, argumentText);
        addLocalMessage(result);
        return true;
      }

      if (command.id === 'effort') {
        if (!argumentText) {
          addLocalMessage('Usage: /effort <low|medium|high>');
          return true;
        }
        const { codexSetEffort } = await import('$lib/tauri/commands');
        const result = await codexSetEffort(agentId, argumentText);
        addLocalMessage(result);
        return true;
      }

      if (command.id === 'approval') {
        if (!argumentText) {
          addLocalMessage('Usage: /approval <never|on-failure|on-request|untrusted>');
          return true;
        }
        const { codexSetApproval } = await import('$lib/tauri/commands');
        const result = await codexSetApproval(agentId, argumentText);
        addLocalMessage(result);
        return true;
      }

      if (command.id === 'goal') {
        const { codexSetGoal } = await import('$lib/tauri/commands');
        const result = await codexSetGoal(agentId, argumentText || undefined);
        addLocalMessage(result);
        return true;
      }

      if (command.id === 'fork') {
        const { codexForkThread } = await import('$lib/tauri/commands');
        const result = await codexForkThread(agentId);
        addLocalMessage(result);
        return true;
      }

      if (command.id === 'rollback') {
        const numTurns = parseInt(argumentText || '1', 10);
        if (isNaN(numTurns) || numTurns < 1) {
          addLocalMessage('Usage: /rollback <number>');
          return true;
        }
        const { codexRollback } = await import('$lib/tauri/commands');
        const result = await codexRollback(agentId, numTurns);
        addLocalMessage(result);
        return true;
      }

      if (command.id === 'shell') {
        if (!argumentText) {
          addLocalMessage('Usage: /shell <command>');
          return true;
        }
        const { codexShellCommand } = await import('$lib/tauri/commands');
        const result = await codexShellCommand(agentId, argumentText);
        addLocalMessage(result);
        return true;
      }
    } catch (e: unknown) {
      addLocalMessage(`Command failed: ${extractErrorMessage(e)}`);
      return true;
    }

    return false;
  }

  // --- Send message ---

  // --- Slash command palette ---

  let showCommandPalette = $state(false);
  let selectedSlashIndex = $state(0);
  const SLASH_GROUP_ORDER = ['general', 'anthropic', 'codex'] as const;
  let trimmedInput = $derived(inputText.trimStart());
  let inputIsSlash = $derived(trimmedInput.startsWith('/'));
  let slashQueryToken = $derived(
    inputIsSlash ? trimmedInput.split(/\s+/)[0].toLowerCase() : '',
  );
  let slashHasArguments = $derived(
    inputIsSlash && trimmedInput.includes(' '),
  );
  let filteredCommands = $derived.by(() => {
    if (!showCommandPalette || !inputIsSlash) return [];
    return filterSlashCommands(slashQueryToken, {
      anthropic: isAnthropicAgent,
      codex: isCodexAgent,
    });
  });

  async function handleSend() {
    const text = inputText.trim();
    if (!text) return;

    // Slash commands work even while agent is streaming
    if (text.startsWith('/')) {
      inputText = '';
      showCommandPalette = false;
      selectedSlashIndex = 0;
      const handled = await handleSlashCommand(text);
      if (handled) return;
      return;
    } else {
      if (sending) return; // only block non-slash messages
      inputText = '';
    }

    // Reset textarea height after clearing input
    if (textareaEl) textareaEl.style.height = 'auto';

    sending = true;

    appendMessageImmediate({
      type: 'user',
      content: text,
      timestamp: Date.now(),
    });
    currentTurnHadStreamError = false;

    try {
      const imagesToSend = pendingImages.length > 0 ? [...pendingImages] : undefined;
      pendingImages = [];
      await sendMessage(agentId, text, imagesToSend);
    } catch (err: unknown) {
      appendTurnErrorMessage(`Failed to send: ${extractErrorMessage(err)}`);
    } finally {
      sending = false;
    }
  }

  function handlePaste(e: ClipboardEvent) {
    const items = e.clipboardData?.items;
    if (!items) return;
    for (const item of items) {
      if (item.type.startsWith('image/')) {
        e.preventDefault();
        const file = item.getAsFile();
        if (file) addImageFile(file);
      }
    }
  }

  function handleDrop(e: DragEvent) {
    e.preventDefault();
    const files = e.dataTransfer?.files;
    if (!files) return;
    for (const file of files) {
      if (file.type.startsWith('image/')) {
        addImageFile(file);
      }
    }
  }

  function handleDragOver(e: DragEvent) {
    e.preventDefault();
    if (e.dataTransfer) {
      e.dataTransfer.dropEffect = 'copy';
    }
  }

  async function addImageFile(file: File) {
    if (file.size > MAX_IMAGE_SIZE) {
      addLocalMessage(`Image too large (${(file.size / 1_000_000).toFixed(1)}MB). Max 5MB.`);
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result as string;
      const base64 = result.split(',')[1];
      const mediaType = file.type || 'image/png';
      pendingImages = [...pendingImages, { data: base64, media_type: mediaType, filename: file.name }];
    };
    reader.readAsDataURL(file);
  }

  function removeImage(index: number) {
    pendingImages = pendingImages.filter((_, i) => i !== index);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (showCommandPalette && filteredCommands.length > 0) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        selectedSlashIndex = (selectedSlashIndex + 1) % filteredCommands.length;
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        selectedSlashIndex = (selectedSlashIndex - 1 + filteredCommands.length) % filteredCommands.length;
        return;
      }
      if (e.key === 'Tab') {
        e.preventDefault();
        applySlashCommandSelection(filteredCommands[selectedSlashIndex] ?? filteredCommands[0]);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        showCommandPalette = false;
        return;
      }
      if (e.key === 'Enter' && !e.shiftKey) {
        const selected = filteredCommands[selectedSlashIndex] ?? filteredCommands[0];
        const isExactMatch =
          selected
          && (selected.cmd === slashQueryToken || selected.aliases?.includes(slashQueryToken as `/${string}`));
        if (selected && !slashHasArguments && !isExactMatch) {
          e.preventDefault();
          applySlashCommandSelection(selected);
          return;
        }
      }
    }

    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  }

  let textareaEl: HTMLTextAreaElement | undefined = $state();

  let resizeRAF: number | null = null;
  function autoResize() {
    if (resizeRAF) return;
    resizeRAF = requestAnimationFrame(() => {
      resizeRAF = null;
      if (!textareaEl) return;
      textareaEl.style.height = 'auto';
      textareaEl.style.height = Math.min(textareaEl.scrollHeight, 200) + 'px';
    });
  }

  // --- HIL / Approval responses ---

  async function submitHilResponse(msgIndex: number, msg: ChatMessage) {
    const response = hilInputs[msgIndex] ?? '';
    if (!response.trim() && (!msg.options || msg.options.length === 0)) return;

    try {
      await respondToHil(agentId, response);
      msg.resolved = true;
      messages = messages; // trigger reactivity
      if (msg.hilId) resolveAttention(msg.hilId);
    } catch (err) {
      console.error('HIL response failed:', err);
      msg.error = 'Failed to send response. Try again.';
      messages = messages;
    }
  }

  async function submitHilOption(msgIndex: number, msg: ChatMessage, optionIndex: number) {
    try {
      await respondToHil(agentId, msg.options![optionIndex], optionIndex);
      msg.resolved = true;
      messages = messages;
      if (msg.hilId) resolveAttention(msg.hilId);
    } catch (err) {
      console.error('HIL option response failed:', err);
      msg.error = 'Failed to send response. Try again.';
      messages = messages;
    }
  }

  async function handleApproval(msg: ChatMessage, approved: boolean) {
    if (!msg.approvalToolCallId) return;
    try {
      await respondToApproval(agentId, msg.approvalToolCallId, approved);
      msg.resolved = true;
      messages = messages;
      resolveAttention(msg.approvalToolCallId);
    } catch (err) {
      console.error('Approval response failed:', err);
    }
  }

  // --- Event listeners ---

  function setupListeners() {
    registerUnlistener(listenAgentStream((event: StreamEvent) => {
      if (event.agent_id !== agentId) return;

      switch (event.event_type) {
        case 'Token': {
          const token = (event.data.text as string) ?? '';
          streamingContent += token;
          scheduleStreamRender();
          break;
        }
        case 'ToolCall': {
          if (streamingContent) {
            appendMessage({
              type: 'agent',
              content: streamingContent,
              timestamp: Date.now(),
            });
            streamingContent = '';
            flushStreamRender();
          }
          const name = (event.data.tool as string) ?? 'unknown';
          const toolCallId = typeof event.data.tool_call_id === 'string' ? event.data.tool_call_id : undefined;
          const args = event.data.arguments;
          const result = event.data.result;
          const status = typeof event.data.status === 'string'
            ? event.data.status as 'done' | 'partial' | 'failed'
            : inferToolStatus(typeof result === 'string' ? result : result != null ? JSON.stringify(result) : '');
          const existingIndex = findActionIndex(toolCallId, name, true);
          const output = typeof args === 'string'
            ? args
            : args != null
              ? JSON.stringify(args)
              : typeof result === 'string'
                ? result
                : result != null
                  ? JSON.stringify(result)
                  : '';
          if (result) {
            if (existingIndex !== -1) {
              batchUpdateAction(existingIndex, {
                toolStatus: status,
                toolOutput: typeof result === 'string' ? result : JSON.stringify(result),
              }, name, result);
            } else {
              appendMessage({
                type: 'action',
                content: '',
                timestamp: Date.now(),
                toolCallId,
                toolName: name,
                toolStatus: status,
                toolOutput: typeof result === 'string' ? result : JSON.stringify(result),
                actionPreview: buildActionPreview(name, result),
              });
            }
          } else if (existingIndex === -1) {
            appendMessage({
              type: 'action',
              content: '',
              timestamp: Date.now(),
              toolCallId,
              toolName: name,
              toolStatus: 'running',
              toolOutput: output ? String(output) : undefined,
              actionPreview: buildActionPreview(name, output ? String(output) : undefined),
            });
          }
          break;
        }
        case 'ToolResult': {
          const name = (event.data.tool as string) ?? 'unknown';
          const toolCallId = typeof event.data.tool_call_id === 'string' ? event.data.tool_call_id : undefined;
          const result = event.data.result;
          const durationMs = typeof event.data.duration_ms === 'number' ? event.data.duration_ms : undefined;
          const status = typeof event.data.status === 'string'
            ? event.data.status as 'done' | 'partial' | 'failed'
            : inferToolStatus(typeof result === 'string' ? result : result != null ? JSON.stringify(result) : '');
          const existingIndex = findActionIndex(toolCallId, name, true);
          if (existingIndex !== -1) {
            batchUpdateAction(existingIndex, {
              toolStatus: status,
              toolOutput: typeof result === 'string' ? result : result != null ? JSON.stringify(result) : '',
              toolDuration: durationMs,
            }, name, result);
          } else {
            appendMessage({
              type: 'action',
              content: '',
              timestamp: Date.now(),
              toolCallId,
              toolName: name,
              toolStatus: status,
              toolDuration: durationMs,
              toolOutput: typeof result === 'string' ? result : result != null ? JSON.stringify(result) : '',
              actionPreview: buildActionPreview(name, result),
            });
          }
          break;
        }
        case 'Thinking': {
          const thinkingText = (event.data.text as string) ?? '';
          const lastMsg = messages[messages.length - 1];
          if (lastMsg && lastMsg.type === 'thinking') {
            // Accumulate thinking text locally -- the $effect on
            // streamingContent/messages.length handles scroll.
            // We batch the reactivity trigger with the next flush.
            lastMsg.content += thinkingText;
            pendingMutations.push({ kind: 'update', index: messages.length - 1, patch: {} });
            scheduleMutationFlush();
          } else {
            appendMessage({
              type: 'thinking',
              content: thinkingText,
              timestamp: Date.now(),
            });
          }
          break;
        }
        case 'A2AMessage': {
          const fromName = (event.data.from_name as string) ?? 'another agent';
          const preview = (event.data.content_preview as string) ?? '';
          appendStatusMessage(
            preview
              ? `Direct message from ${fromName}: ${preview}`
              : `Direct message from ${fromName}`,
            'message',
          );
          break;
        }
        case 'ConversationUpdated': {
          getConversation(agentId)
            .then((conversation) => {
              const converted = convertConversation(conversation);
              const reconciled = reconcileConversationMessages(converted, messages);
              if (reconciled.length > MAX_CHAT_MESSAGES) {
                console.warn(`[ChatTile] Message cap reached for agent ${agentId}, trimming oldest`);
                messages = trimMessages(reconciled);
              } else {
                messages = trimMessages(reconciled);
              }
            })
            .catch((err) => {
              console.error('Failed to refresh conversation:', err);
            });
          break;
        }
        case 'Done': {
          if (streamingContent) {
            appendMessage({
              type: 'agent',
              content: streamingContent,
              timestamp: Date.now(),
            });
            streamingContent = '';
            flushStreamRender();
          }
          if (!currentTurnHadStreamError) {
            clearLocalTurnErrors();
          }
          currentTurnHadStreamError = false;
          // Set all running actions as done -- batch the update
          for (let i = 0; i < messages.length; i++) {
            if (messages[i].type === 'action' && messages[i].toolStatus === 'running') {
              messages[i].toolStatus = 'done';
              pendingMutations.push({ kind: 'update', index: i, patch: {} });
            }
          }
          if (pendingMutations.length > 0) {
            scheduleMutationFlush();
          }
          break;
        }
        case 'Error': {
          streamingContent = '';
          flushStreamRender();
          currentTurnHadStreamError = true;
          appendTurnErrorMessage(`Error: ${streamErrorMessage(event.data)}`);
          break;
        }
        case 'SessionMemoryUpdated': {
          appendMessage({
            type: 'system',
            content: 'Session memory updated.',
            timestamp: Date.now(),
          });
          break;
        }
      }
    }));

    // SDK agents now also emit to agent-stream (dual emission from agent_sdk.rs)
    // so no separate listenAgentSdkStream needed — all tiles use the unified stream.

    registerUnlistener(listenHilQuestion((event: HilQuestionEvent) => {
      if (event.agent_id !== agentId) return;
      appendMessageImmediate({
        type: 'hil',
        content: event.question,
        timestamp: Date.now(),
        hilId: hilAttentionId(event.agent_id),
        options: event.options ?? undefined,
        resolved: false,
      });
    }));

    registerUnlistener(listenHilApproval((event: HilApprovalEvent) => {
      if (event.agent_id !== agentId) return;
      appendMessageImmediate({
        type: 'approval',
        content: `${event.tool_description}\n${event.arguments_preview}`,
        timestamp: Date.now(),
        approvalTool: event.tool_name,
        approvalToolCallId: event.tool_call_id,
        resolved: false,
      });
    }));

    registerUnlistener(listenHilVeto((event: HilVetoEvent) => {
      if (event.agent_id !== agentId) return;
      appendMessageImmediate({
        type: 'approval',
        content: event.detail,
        timestamp: Date.now(),
        approvalTool: event.tool_name,
        approvalToolCallId: event.tool_call_id,
        approvalTimeout: event.timeout_seconds,
        resolved: false,
      });
    }));

  }

  // --- Auto-scroll effect ---

  $effect(() => {
    // Track length + rendered output to trigger scroll (not raw streamingContent
    // which updates 20x/sec — renderedStreamHtml only updates ~5x/sec)
    void messages.length;
    void renderedStreamHtml;
    scrollToBottom();
  });

  // --- Lifecycle ---

  onMount(async () => {
    try {
      const [conversation, usage] = await Promise.all([
        getConversation(agentId),
        getAgentUsage(agentId).catch(() => null),
      ]);
      const converted = convertConversation(conversation);
      messages = trimMessages(converted);
      if (usage?.model_name) {
        currentModel = usage.model_name;
      }
    } catch (err) {
      console.error('Failed to load conversation:', err);
    }

    setupListeners();
    scrollToBottom();
    if (textareaEl) textareaEl.focus();
  });

  onDestroy(() => {
    destroyed = true;
    if (renderThrottleId !== null) clearTimeout(renderThrottleId);
    pendingImages = [];
    localMessages = [];
    for (const unlisten of unlisteners) {
      unlisten();
    }
    unlisteners = [];
  });

  // --- Derived ---

  function dotColor(status?: string): string {
    switch (status) {
      case 'running': return 'var(--agent-accent, #06b6d4)';
      case 'done': return 'var(--success-accent, #22c55e)';
      case 'partial': return 'var(--warning-accent, #f59e0b)';
      case 'failed': return 'var(--alert-accent, #ef4444)';
      default: return 'var(--warning-accent, #f59e0b)';
    }
  }

  function statusIcon(status?: string): string {
    switch (status) {
      case 'done': return '\u2713';
      case 'partial': return '\u26A0';
      case 'failed': return '\u2717';
      case 'running': return '\u2026';
      default: return '\u2026';
    }
  }

  function truncatePath(path: string, maxLen = 40): string {
    if (path.length <= maxLen) return path;
    return '\u2026' + path.slice(path.length - maxLen + 1);
  }

</script>

<div class="chat-tile">
  <!-- Message area -->
  <div class="messages" bind:this={scrollContainer}>
    {#if hasMoreAbove}
      <button class="load-more-sentinel" onclick={loadMoreMessages}>
        &#8593; Load older messages
      </button>
    {/if}
    {#each visibleMessages as { msg, globalIndex: i } (msg.timestamp + '-' + i)}
      {#if msg.type === 'user'}
        <div class="msg msg-user">
          <div class="msg-border user-border"></div>
          <div class="msg-body">
            {msg.content}
            {#if msg.contentBlocks}
              {#each msg.contentBlocks as block}
                {#if block.type === 'image' && block.data}
                  <img class="chat-image" src="data:{block.media_type};base64,{block.data}" alt="Attachment" />
                {/if}
              {/each}
            {/if}
          </div>
        </div>

      {:else if msg.type === 'agent'}
        <div class="msg msg-agent">
          <div class="msg-border agent-border"></div>
          <div class="msg-body markdown-content">{@html msg.renderedHtml ?? renderMarkdown(msg.content)}</div>
        </div>

      {:else if msg.type === 'system'}
        <div class="msg msg-system">
          <span class="system-dash">&mdash;</span>
          <span class="system-text">{msg.content}</span>
          <span class="system-dash">&mdash;</span>
        </div>

      {:else if msg.type === 'action'}
        <div class="msg msg-action">
          <button
            class="action-toggle"
            onclick={() => toggleAction(i)}
            title={msg.toolOutput ? 'Toggle output' : undefined}
            disabled={!msg.toolOutput}
          >
            {#if msg.toolOutput}
              <span class="action-chevron" class:expanded={expandedActions.has(i)}>&#9656;</span>
            {:else}
              <span class="action-dot" style="color: {dotColor(msg.toolStatus)}">&#9679;</span>
            {/if}
          </button>
          <span class="action-dot-inline" style="color: {dotColor(msg.toolStatus)}">&#9679;</span>
          <span class="action-name">{formatToolDisplayName(msg.toolName)}</span>
          {#if msg.actionPreview}
            <span class="action-preview">{msg.actionPreview}</span>
          {/if}
          <span class="action-status">{statusIcon(msg.toolStatus)}</span>
          {#if msg.toolDuration != null}
            <span class="action-duration">{msg.toolDuration}ms</span>
          {/if}
          {#if expandedActions.has(i) && msg.toolOutput}
            <div class="action-output">
              <pre>{msg.toolOutput}</pre>
            </div>
          {/if}
        </div>

      {:else if msg.type === 'thinking'}
        <div class="msg msg-thinking">
          <button class="thinking-toggle" onclick={() => {
            if (expandedActions.has(i)) expandedActions.delete(i);
            else expandedActions.add(i);
            expandedActions = new Set(expandedActions);
          }}>
            <span class="thinking-indicator">Thinking</span>
            <span class="thinking-chevron" class:expanded={expandedActions.has(i)}>&#9656;</span>
          </button>
          {#if expandedActions.has(i)}
            <div class="thinking-text">{msg.content}</div>
          {/if}
        </div>

      {:else if msg.type === 'status'}
        <div class="msg msg-status" data-tone={msg.statusTone ?? 'neutral'}>
          <span class="status-line"></span>
          <span class="status-pill">{msg.content}</span>
          <span class="status-line"></span>
        </div>

      {:else if msg.type === 'hil'}
        <div class="msg msg-hil" class:resolved={msg.resolved}>
          <div class="msg-border hil-border"></div>
          <div class="hil-body">
            <div class="hil-label">Agent needs input</div>
            <div class="hil-question">{msg.content}</div>
            {#if !msg.resolved}
              {#if msg.options && msg.options.length > 0}
                <div class="hil-options">
                  {#each msg.options as option, oi}
                    <button
                      class="hil-option-btn"
                      onclick={() => submitHilOption(i, msg, oi)}
                    >
                      {option}
                    </button>
                  {/each}
                </div>
              {:else}
                <div class="hil-input-row">
                  <input
                    type="text"
                    class="hil-input"
                    placeholder="Type your response..."
                    bind:value={hilInputs[i]}
                    onkeydown={(e) => { if (e.key === 'Enter') submitHilResponse(i, msg); }}
                  />
                  <button
                    class="hil-submit"
                    onclick={() => submitHilResponse(i, msg)}
                  >
                    Submit
                  </button>
                </div>
              {/if}
              {#if msg.error}
                <div class="hil-error">{msg.error}</div>
              {/if}
            {:else}
              <div class="hil-resolved">Resolved</div>
            {/if}
          </div>
        </div>

      {:else if msg.type === 'approval'}
        <div class="msg msg-approval" class:resolved={msg.resolved}>
          <div class="msg-border approval-border"></div>
          <div class="approval-body">
            <div class="approval-label">Approval required</div>
            {#if msg.approvalTool}
              <div class="approval-tool">{msg.approvalTool}</div>
            {/if}
            <div class="approval-detail">{msg.content}</div>
            {#if msg.approvalTimeout}
              <div class="approval-timeout">Auto-proceeds in {msg.approvalTimeout}s unless denied</div>
            {/if}
            {#if !msg.resolved}
              <div class="approval-actions">
                <button
                  class="approval-btn approve"
                  onclick={() => handleApproval(msg, true)}
                >
                  Approve
                </button>
                <button
                  class="approval-btn deny"
                  onclick={() => handleApproval(msg, false)}
                >
                  Deny
                </button>
              </div>
            {:else}
              <div class="hil-resolved">Resolved</div>
            {/if}
          </div>
        </div>
      {/if}
    {/each}

    <!-- Streaming content (in-progress agent response) -->
    {#if streamingContent}
      <div class="msg msg-agent streaming">
        <div class="msg-border agent-border"></div>
        <div class="msg-body markdown-content">{@html renderedStreamHtml}<span class="cursor-blink">|</span></div>
      </div>
    {/if}

    <!-- Local command feedback messages -->
    {#each localMessages as msg (msg.timestamp)}
      <div class="local-message">{msg.text}</div>
    {/each}
  </div>

  <!-- Input bar -->
  <div class="input-bar">
    {#if showModelPicker}
      <select
        class="model-select"
        bind:value={currentModel}
        onchange={async (e) => {
          const target = e.target as HTMLSelectElement;
          try {
            await updateAnthropicSettings(agentId, target.value);
            currentModel = target.value;
            addLocalMessage(`Model switched to: ${target.value}`);
          } catch (err) {
            addLocalMessage(`Failed to switch model: ${err}`);
          }
        }}
      >
        {#if isAnthropicAgent}
          <option value="opus4.5">Opus 4.5</option>
          <option value="opus4.6">Opus 4.6</option>
          <option value="sonnet4.5">Sonnet 4.5</option>
          <option value="sonnet4.6">Sonnet 4.6</option>
        {:else}
          <option value={currentModel}>{currentModel || 'default'}</option>
        {/if}
      </select>
    {/if}
    <div class="input-stack">
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="input-wrapper" ondrop={handleDrop} ondragover={handleDragOver}>
        {#if showCommandPalette}
          <div class="command-palette">
            {#if filteredCommands.length > 0}
              {#each SLASH_GROUP_ORDER as group}
                {@const groupCommands = filteredCommands.filter((cmd) => cmd.group === group)}
                {#if groupCommands.length > 0}
                  <div class="cmd-group-label">{slashGroupLabel(group)}</div>
                  {#each groupCommands as cmd}
                    {@const absoluteIndex = filteredCommands.findIndex((candidate) => candidate.id === cmd.id)}
                    <button
                      class="cmd-item"
                      class:active={absoluteIndex === selectedSlashIndex}
                      onclick={() => applySlashCommandSelection(cmd)}
                      onmouseenter={() => { selectedSlashIndex = absoluteIndex; }}
                    >
                      <span class="cmd-name">{cmd.cmd}</span>
                      <span class="cmd-args">{cmd.args}</span>
                      <span class="cmd-desc">{cmd.desc}</span>
                    </button>
                  {/each}
                {/if}
              {/each}
            {:else}
              <div class="cmd-empty">No matching commands</div>
            {/if}
          </div>
        {/if}
        {#if pendingImages.length > 0}
          <div class="image-preview-strip">
            {#each pendingImages as img, i}
              <div class="image-preview">
                <img src="data:{img.media_type};base64,{img.data}" alt={img.filename ?? 'Pasted image'} />
                <button class="remove-image" onclick={() => removeImage(i)}>×</button>
              </div>
            {/each}
          </div>
        {/if}
        <textarea
          class="input-field"
          placeholder={showStopButton ? 'Type /commands while agent works...' : 'Send a message or type / for commands...'}
          bind:value={inputText}
          bind:this={textareaEl}
          onkeydown={handleKeydown}
          onpaste={handlePaste}
          oninput={() => {
            showCommandPalette = inputIsSlash && !slashHasArguments;
            selectedSlashIndex = 0;
            autoResize();
          }}
          rows="1"
          spellcheck="true"
        ></textarea>
      </div>
    </div>
    {#if showStopButton}
      <button
        class="stop-btn"
        onclick={async () => { try { await interruptAgent(agentId); } catch (_) {} }}
        title="Stop agent (interrupt)"
      >
        &#9632;
      </button>
    {:else}
      <button
        class="send-btn"
        onclick={handleSend}
        disabled={!inputText.trim()}
        title="Send (Enter)"
      >
        &#9654;
      </button>
    {/if}
  </div>
</div>

<style>
  .chat-tile {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    overflow: hidden;
  }

  /* Messages container */
  .messages {
    flex: 1;
    overflow-y: auto;
    padding: 12px 16px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .load-more-sentinel {
    background: none;
    border: 1px solid var(--surface-border);
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 0.786rem;
    padding: 6px 12px;
    text-align: center;
    margin-bottom: 4px;
    transition: color var(--transition-speed, 200ms) ease, border-color var(--transition-speed, 200ms) ease;
  }

  .load-more-sentinel:hover {
    color: var(--text-primary);
    border-color: var(--text-muted);
  }

  /* Base message */
  .msg {
    max-width: 100%;
    word-wrap: break-word;
    overflow-wrap: break-word;
  }

  /* Agent messages: left-aligned with accent border */
  .msg-agent {
    display: flex;
    gap: 0;
    padding: 8px 0;
    max-width: 85%;
  }

  /* User messages: right-aligned, distinct surface */
  .msg-user {
    display: flex;
    gap: 0;
    padding: 8px 0;
    max-width: 85%;
    margin-left: auto;
    flex-direction: row-reverse;
  }

  .msg-border {
    width: 3px;
    flex-shrink: 0;
    border-radius: 2px;
    margin-right: 10px;
  }

  .msg-user .msg-border {
    margin-right: 0;
    margin-left: 10px;
  }

  .agent-border {
    background: var(--agent-accent, #06b6d4);
  }

  .user-border {
    background: var(--text-secondary, #94a3b8);
  }

  .msg-body {
    font-size: 0.929rem;
    line-height: 1.6;
    color: var(--text-primary);
    white-space: pre-wrap;
  }

  .msg-user .msg-body {
    background: color-mix(in srgb, var(--surface-border) 30%, var(--card-bg));
    padding: 6px 10px;
    border-radius: var(--border-radius, 8px);
    border-top-right-radius: 2px;
  }

  .msg-agent .msg-body {
    padding: 6px 10px;
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 6%, var(--card-bg));
    border-radius: var(--border-radius, 8px);
    border-top-left-radius: 2px;
  }

  .msg-agent.streaming .msg-body {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 4%, var(--card-bg));
  }

  /* Markdown rendered content */
  .msg-body.markdown-content {
    white-space: normal;
  }

  .markdown-content :global(p) {
    margin: 0 0 8px 0;
  }

  .markdown-content :global(p:last-child) {
    margin-bottom: 0;
  }

  .markdown-content :global(h1),
  .markdown-content :global(h2),
  .markdown-content :global(h3),
  .markdown-content :global(h4) {
    color: var(--text-primary);
    margin: 12px 0 6px 0;
    line-height: 1.3;
  }

  .markdown-content :global(h1) { font-size: 1.143rem; }
  .markdown-content :global(h2) { font-size: 1.071rem; }
  .markdown-content :global(h3) { font-size: 1rem; }
  .markdown-content :global(h4) { font-size: 0.929rem; }

  .markdown-content :global(strong) {
    font-weight: 600;
    color: var(--text-primary);
  }

  .markdown-content :global(em) {
    font-style: italic;
  }

  .markdown-content :global(code) {
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    background: color-mix(in srgb, var(--canvas-bg) 70%, var(--card-bg));
    padding: 1px 5px;
    border-radius: 3px;
    color: var(--agent-accent, #06b6d4);
  }

  .markdown-content :global(pre) {
    background: color-mix(in srgb, var(--canvas-bg) 80%, var(--card-bg));
    border: var(--border-width) solid var(--surface-border);
    border-radius: 6px;
    padding: 10px 12px;
    margin: 8px 0;
    overflow-x: auto;
  }

  .markdown-content :global(pre code) {
    background: none;
    padding: 0;
    border-radius: 0;
    color: var(--text-primary);
    font-size: 0.857rem;
    line-height: 1.5;
  }

  .markdown-content :global(ul),
  .markdown-content :global(ol) {
    margin: 6px 0;
    padding-left: 20px;
  }

  .markdown-content :global(li) {
    margin: 2px 0;
  }

  .markdown-content :global(blockquote) {
    border-left: 3px solid var(--agent-accent, #06b6d4);
    margin: 8px 0;
    padding: 4px 12px;
    color: var(--text-secondary);
    background: color-mix(in srgb, var(--canvas-bg) 50%, var(--card-bg));
    border-radius: 0 4px 4px 0;
  }

  .markdown-content :global(table) {
    border-collapse: collapse;
    margin: 8px 0;
    font-size: 0.857rem;
    width: 100%;
  }

  .markdown-content :global(th),
  .markdown-content :global(td) {
    border: var(--border-width) solid var(--surface-border);
    padding: 4px 8px;
    text-align: left;
  }

  .markdown-content :global(th) {
    background: color-mix(in srgb, var(--canvas-bg) 60%, var(--card-bg));
    font-weight: 600;
    color: var(--text-primary);
  }

  .markdown-content :global(hr) {
    border: none;
    border-top: var(--border-width) solid var(--surface-border);
    margin: 10px 0;
  }

  .markdown-content :global(a) {
    color: var(--agent-accent, #06b6d4);
    text-decoration: none;
  }

  .markdown-content :global(a:hover) {
    text-decoration: underline;
  }

  /* highlight.js token colors — theme-aware */
  .markdown-content :global(.hljs-keyword),
  .markdown-content :global(.hljs-selector-tag) {
    color: #c678dd;
  }

  .markdown-content :global(.hljs-string),
  .markdown-content :global(.hljs-addition) {
    color: #98c379;
  }

  .markdown-content :global(.hljs-number),
  .markdown-content :global(.hljs-literal) {
    color: #d19a66;
  }

  .markdown-content :global(.hljs-comment),
  .markdown-content :global(.hljs-quote) {
    color: var(--text-muted);
    font-style: italic;
  }

  .markdown-content :global(.hljs-function),
  .markdown-content :global(.hljs-title) {
    color: #61afef;
  }

  .markdown-content :global(.hljs-built_in) {
    color: #e6c07b;
  }

  .markdown-content :global(.hljs-type),
  .markdown-content :global(.hljs-class .hljs-title) {
    color: #e6c07b;
  }

  .markdown-content :global(.hljs-attr),
  .markdown-content :global(.hljs-variable) {
    color: #d19a66;
  }

  .markdown-content :global(.hljs-deletion) {
    color: #e06c75;
  }

  .markdown-content :global(.hljs-meta) {
    color: var(--text-muted);
  }

  /* System messages */
  .msg-system {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 4px 0;
  }

  .system-dash {
    color: var(--text-muted);
    font-size: 0.786rem;
    opacity: 0.5;
  }

  .system-text {
    font-size: 0.786rem;
    color: var(--text-muted);
    font-style: italic;
  }

  /* Thinking messages */
  .msg-thinking {
    display: flex;
    flex-direction: column;
    padding: 2px 0 2px 4px;
    font-size: 0.857rem;
    color: var(--text-muted);
    font-style: italic;
  }

  .thinking-toggle {
    display: flex;
    align-items: center;
    gap: 4px;
    background: none;
    border: none;
    cursor: pointer;
    padding: 2px 4px;
    border-radius: 3px;
    width: fit-content;
  }

  .thinking-toggle:hover {
    background: rgba(255, 255, 255, 0.05);
  }

  .thinking-indicator {
    font-family: var(--mono-family, monospace);
    font-size: 0.714rem;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--warning-accent, #f59e0b);
    font-weight: 600;
    flex-shrink: 0;
  }

  .thinking-chevron {
    font-size: 0.714rem;
    color: var(--text-muted);
    transition: transform 150ms;
    display: inline-block;
  }

  .thinking-chevron.expanded {
    transform: rotate(90deg);
  }

  .thinking-text {
    font-size: 0.857rem;
    color: var(--text-muted);
    padding: 4px 8px 4px 16px;
    white-space: pre-wrap;
    max-height: 200px;
    overflow-y: auto;
    border-left: 2px solid var(--warning-accent, #f59e0b);
    margin-top: 4px;
    opacity: 0.7;
  }

  .thinking-text {
    font-size: 0.857rem;
    color: var(--text-muted);
    line-height: 1.4;
    white-space: pre-wrap;
  }

  /* Action timeline */
  .msg-action {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 0 2px 4px;
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    color: var(--text-secondary);
    flex-wrap: wrap;
  }

  .action-toggle {
    background: none;
    border: none;
    cursor: pointer;
    padding: 0 2px;
    font-size: 0.714rem;
    line-height: 1;
    color: var(--text-muted);
    display: flex;
    align-items: center;
  }

  .action-toggle:disabled {
    cursor: default;
  }

  .action-chevron {
    display: inline-block;
    transition: transform 120ms ease;
    font-size: 0.786rem;
  }

  .action-chevron.expanded {
    transform: rotate(90deg);
  }

  .action-dot-inline {
    font-size: 0.571rem;
    line-height: 1;
  }

  .action-dot {
    font-size: 0.571rem;
  }

  .action-name {
    color: var(--text-primary);
    font-weight: 600;
    text-transform: capitalize;
  }

  .action-preview {
    color: var(--text-muted);
    font-size: 0.714rem;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .action-status {
    color: var(--text-muted);
    font-size: 0.786rem;
  }

  .action-duration {
    color: var(--text-muted);
    font-size: 0.714rem;
  }

  .action-output {
    width: 100%;
    margin-top: 4px;
    margin-left: 20px;
    padding: 6px 8px;
    background: color-mix(in srgb, var(--canvas-bg) 60%, var(--card-bg));
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    overflow-x: auto;
  }

  .action-output pre {
    font-family: var(--mono-family, monospace);
    font-size: 0.786rem;
    color: var(--text-secondary);
    white-space: pre-wrap;
    margin: 0;
    max-height: 200px;
    overflow-y: auto;
  }

  /* Status dividers */
  .msg-status {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 6px 0;
  }

  .status-line {
    flex: 1;
    max-width: 80px;
    height: 1px;
    background: var(--surface-border);
  }

  .status-pill {
    font-family: var(--mono-family, monospace);
    font-size: 0.714rem;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    white-space: nowrap;
  }

  .msg-status[data-tone='control'] .status-pill {
    color: var(--text-primary);
    text-transform: none;
    border: 1px solid color-mix(in srgb, var(--agent-accent, #06b6d4) 35%, transparent);
    border-radius: 999px;
    padding: 4px 10px;
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 10%, transparent);
  }

  .msg-status[data-tone='message'] .status-pill {
    color: var(--text-secondary);
    text-transform: none;
    border: 1px solid color-mix(in srgb, var(--success-accent, #22c55e) 28%, transparent);
    border-radius: 999px;
    padding: 4px 10px;
    background: color-mix(in srgb, var(--success-accent, #22c55e) 8%, transparent);
  }

  /* HIL prompts */
  .msg-hil {
    display: flex;
    gap: 0;
    padding: 8px 0;
  }

  .hil-border {
    background: var(--agent-accent, #06b6d4);
  }

  .hil-body {
    display: flex;
    flex-direction: column;
    gap: 6px;
    flex: 1;
  }

  .hil-label {
    font-family: var(--mono-family, monospace);
    font-size: 0.714rem;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--agent-accent, #06b6d4);
    font-weight: 600;
  }

  .hil-question {
    font-size: 0.929rem;
    color: var(--text-primary);
    line-height: 1.5;
  }

  .hil-options {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 2px;
  }

  .hil-option-btn {
    font-size: 0.857rem;
    font-family: var(--mono-family, monospace);
    padding: 4px 12px;
    border: var(--border-width) solid var(--agent-accent, #06b6d4);
    border-radius: var(--border-radius);
    background: transparent;
    color: var(--agent-accent, #06b6d4);
    cursor: pointer;
    transition: background 120ms, color 120ms;
  }

  .hil-option-btn:hover {
    background: var(--agent-accent, #06b6d4);
    color: var(--canvas-bg, #0f172a);
  }

  .hil-input-row {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  .hil-input {
    flex: 1;
    font-size: 0.857rem;
    padding: 4px 8px;
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    background: var(--canvas-bg);
    color: var(--text-primary);
    outline: none;
  }

  .hil-input:focus {
    border-color: var(--agent-accent, #06b6d4);
  }

  .hil-submit {
    font-size: 0.857rem;
    font-family: var(--mono-family, monospace);
    padding: 4px 10px;
    border: var(--border-width) solid var(--agent-accent, #06b6d4);
    border-radius: var(--border-radius);
    background: var(--agent-accent, #06b6d4);
    color: var(--canvas-bg, #0f172a);
    cursor: pointer;
    font-weight: 600;
  }

  .hil-resolved {
    font-size: 0.786rem;
    color: var(--text-muted);
    font-style: italic;
  }

  .hil-error {
    font-size: 0.786rem;
    color: #ef4444;
    margin-top: 4px;
  }

  /* Approval prompts */
  .msg-approval {
    display: flex;
    gap: 0;
    padding: 8px 0;
  }

  .approval-border {
    background: var(--alert-accent, #ef4444);
  }

  .approval-body {
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
  }

  .approval-label {
    font-family: var(--mono-family, monospace);
    font-size: 0.714rem;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--alert-accent, #ef4444);
    font-weight: 600;
  }

  .approval-tool {
    font-family: var(--mono-family, monospace);
    font-size: 0.929rem;
    color: var(--text-primary);
    font-weight: 600;
  }

  .approval-detail {
    font-size: 0.857rem;
    color: var(--text-secondary);
    line-height: 1.4;
    white-space: pre-wrap;
  }

  .approval-timeout {
    font-size: 0.714rem;
    color: var(--warning-accent, #f59e0b);
    font-family: var(--mono-family, monospace);
  }

  .approval-actions {
    display: flex;
    gap: 8px;
    margin-top: 4px;
  }

  .approval-btn {
    font-size: 0.857rem;
    font-family: var(--mono-family, monospace);
    padding: 4px 14px;
    border-radius: var(--border-radius);
    border: var(--border-width) solid;
    cursor: pointer;
    font-weight: 600;
    transition: background 120ms, color 120ms;
  }

  .approval-btn.approve {
    border-color: var(--success-accent, #22c55e);
    background: transparent;
    color: var(--success-accent, #22c55e);
  }

  .approval-btn.approve:hover {
    background: var(--success-accent, #22c55e);
    color: var(--canvas-bg, #0f172a);
  }

  .approval-btn.deny {
    border-color: var(--alert-accent, #ef4444);
    background: transparent;
    color: var(--alert-accent, #ef4444);
  }

  .approval-btn.deny:hover {
    background: var(--alert-accent, #ef4444);
    color: #fff;
  }

  .resolved {
    opacity: 0.6;
  }

  /* Streaming cursor */
  .cursor-blink {
    animation: blink 800ms steps(1) infinite;
    color: var(--agent-accent, #06b6d4);
  }

  @keyframes blink {
    0%, 100% { opacity: 1; }
    50% { opacity: 0; }
  }

  /* Input bar */
  .input-bar {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 8px 12px;
    border-top: var(--border-width) solid var(--surface-border);
    background: var(--card-bg);
    flex-shrink: 0;
  }

  .input-stack {
    display: flex;
    flex: 1;
    min-width: 0;
    flex-direction: column;
    gap: 6px;
  }

  .input-field {
    width: 100%;
    resize: none;
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    background: var(--canvas-bg);
    color: var(--text-primary);
    padding: 8px 10px;
    font-size: 0.929rem;
    line-height: 1.4;
    outline: none;
    min-height: 36px;
    max-height: 200px;
    font-family: inherit;
    overflow-y: auto;
  }

  .input-field:focus {
    border-color: var(--agent-accent, #06b6d4);
  }

  .input-field::placeholder {
    color: var(--text-muted);
  }

  .send-btn {
    width: 36px;
    height: 36px;
    border: none;
    border-radius: var(--border-radius);
    background: var(--agent-accent, #06b6d4);
    color: var(--canvas-bg, #0f172a);
    cursor: pointer;
    font-size: 1rem;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    align-self: center;
    transition: opacity 120ms;
  }

  .send-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .send-btn:not(:disabled):hover {
    opacity: 0.85;
  }

  .stop-btn {
    width: 36px;
    height: 36px;
    border: none;
    border-radius: var(--border-radius);
    background: var(--alert-accent, #ef4444);
    color: var(--canvas-bg, #0f172a);
    cursor: pointer;
    font-size: 1.143rem;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    align-self: center;
    transition: opacity 120ms;
  }

  .stop-btn:hover {
    opacity: 0.85;
  }

  .model-select {
    background: var(--card-bg);
    border: var(--border-width) solid var(--surface-border);
    border-radius: var(--border-radius);
    color: var(--text-secondary);
    font-family: var(--mono-family);
    font-size: 0.786rem;
    padding: 2px 6px;
    cursor: pointer;
    outline: none;
  }

  .model-select:focus {
    border-color: var(--agent-accent);
  }

  .local-message {
    font-family: var(--mono-family);
    font-size: 0.857rem;
    color: var(--text-muted);
    padding: 4px 12px;
    border-left: 2px solid var(--surface-border);
    margin: 4px 0;
  }

  /* Input wrapper for command palette positioning */
  .input-wrapper {
    position: relative;
    flex: 1;
  }

  /* Slash command palette */
  .command-palette {
    position: absolute;
    bottom: 100%;
    left: 0;
    right: 0;
    background: var(--card-bg);
    border: 1px solid var(--surface-border);
    border-radius: 6px;
    padding: 4px;
    margin-bottom: 4px;
    max-height: 200px;
    overflow-y: auto;
    z-index: 10;
  }

  .cmd-group-label {
    padding: 8px 8px 4px;
    color: var(--text-muted);
    font-family: var(--mono-family, monospace);
    font-size: 0.714rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .cmd-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 8px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
    cursor: pointer;
    border-radius: 4px;
    text-align: left;
  }

  .cmd-item:hover {
    background: rgba(255, 255, 255, 0.05);
  }

  .cmd-item.active {
    background: color-mix(in srgb, var(--agent-accent, #06b6d4) 18%, transparent);
    outline: 1px solid color-mix(in srgb, var(--agent-accent, #06b6d4) 30%, transparent);
  }

  .cmd-name {
    color: var(--agent-accent, #06b6d4);
    font-weight: 600;
    min-width: 80px;
  }

  .cmd-args {
    color: var(--text-muted);
    min-width: 60px;
  }

  .cmd-desc {
    color: var(--text-secondary);
    font-size: 0.786rem;
  }

  .cmd-empty {
    padding: 10px 8px;
    color: var(--text-muted);
    font-family: var(--mono-family, monospace);
    font-size: 0.857rem;
  }

  .image-preview-strip {
    display: flex;
    gap: 8px;
    padding: 8px;
    overflow-x: auto;
  }

  .image-preview {
    position: relative;
    flex-shrink: 0;
  }

  .image-preview img {
    max-height: 80px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #333);
  }

  .remove-image {
    position: absolute;
    top: -6px;
    right: -6px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--error-color, #e53e3e);
    color: white;
    border: none;
    cursor: pointer;
    font-size: 0.857rem;
    line-height: 1;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .chat-image {
    max-width: 200px;
    cursor: pointer;
    border-radius: 4px;
    margin-top: 4px;
    display: block;
  }

</style>
