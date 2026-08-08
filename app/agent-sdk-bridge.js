#!/usr/bin/env node

/**
 * Agent SDK Bridge — Node.js sidecar for Holt
 *
 * Communicates with Rust backend via NDJSON over stdio.
 * Uses the Agent SDK query() API with async iterable for multi-turn:
 * - Persistent warm sessions (single CLI process kept alive)
 * - Proper systemPrompt (persona identity)
 * - MCP tools via createSdkMcpServer (Holt intelligence tools)
 * - Bidirectional permission flow via canUseTool callback
 * - Session resume on app restart
 */

import { createInterface } from 'readline';
import {
    query,
    createSdkMcpServer,
    tool,
} from '@anthropic-ai/claude-agent-sdk';
import { z } from 'zod';

// ── State ──────────────────────────────────────────────────────────────

let config = null;
let currentQuery = null;     // Active Query (AsyncGenerator<SDKMessage>)
let inputQueue = null;       // AsyncQueue feeding messages to the query
let messageQueue = [];       // Incoming NDJSON messages queued before query is ready
let isProcessing = false;    // Whether we're currently processing a message
let pendingPermissions = new Map();   // requestId → {resolve, reject}
let nextRequestId = 1;
let sessionId = null;        // Tracked from SDK messages

// ── NDJSON I/O ─────────────────────────────────────────────────────────

function emit(obj) {
    process.stdout.write(JSON.stringify(obj) + '\n');
}

// ── Async Queue ────────────────────────────────────────────────────────
// Feeds SDKUserMessage objects to the query() async iterable.

class AsyncQueue {
    constructor() {
        this.items = [];
        this.resolvers = [];
        this.isDone = false;
    }

    enqueue(item) {
        if (this.isDone) return;
        if (this.resolvers.length > 0) {
            this.resolvers.shift()(item);
        } else {
            this.items.push(item);
        }
    }

    finish() {
        this.isDone = true;
        for (const r of this.resolvers) r(null);
        this.resolvers = [];
    }

    async *[Symbol.asyncIterator]() {
        while (!this.isDone) {
            if (this.items.length > 0) {
                yield this.items.shift();
            } else {
                const item = await new Promise(r => this.resolvers.push(r));
                if (item === null) return;
                yield item;
            }
        }
    }
}

// ── Incoming message handler ───────────────────────────────────────────

const rl = createInterface({ input: process.stdin, terminal: false });

rl.on('line', async (line) => {
    const trimmed = line.trim();
    if (!trimmed) return;

    let msg;
    try {
        msg = JSON.parse(trimmed);
    } catch {
        console.error(`[bridge] Malformed NDJSON (${trimmed.length} chars): ${trimmed.substring(0, 100)}...`);
        return;
    }

    switch (msg.type) {
        case 'init':
            await handleInit(msg);
            break;
        case 'query':
            enqueueUserMessage(msg);
            break;
        case 'tool_response':
            handleToolResponse(msg);
            break;
        case 'permission_response':
            handlePermissionResponse(msg);
            break;
        case 'hil_response':
            handleHilResponse(msg);
            break;
        case 'abort':
            handleAbort();
            break;
        case 'shutdown':
            await handleShutdown();
            break;
    }
});

// ── Holt MCP Tools ───────────────────────────────────────────────────
// Tools relay to Rust's reduced SDK tool registry via NDJSON.
// Handler emits tool_request, waits for tool_response from Rust.

let pendingToolRequests = new Map(); // requestId → {resolve, reject}

function createToolHandler(toolName) {
    return async (args) => {
        const requestId = `tool-${nextRequestId++}`;

        return new Promise((resolve, reject) => {
            const timeoutId = setTimeout(() => {
                if (pendingToolRequests.has(requestId)) {
                    pendingToolRequests.delete(requestId);
                    resolve({ content: [{ type: 'text', text: `Tool ${toolName} timed out` }] });
                }
            }, 30000);

            pendingToolRequests.set(requestId, { resolve, reject, timeoutId });

            emit({
                type: 'tool_request',
                name: toolName,
                args: args || {},
                requestId: requestId,
            });
        });
    };
}

function handleToolResponse(msg) {
    const pending = pendingToolRequests.get(msg.requestId);
    if (!pending) return;
    pendingToolRequests.delete(msg.requestId);
    clearTimeout(pending.timeoutId);

    const resultText = typeof msg.result === 'string' ? msg.result : JSON.stringify(msg.result);
    pending.resolve({ content: [{ type: 'text', text: resultText }] });
}

// Holt intelligence tools with Zod schemas (required by createSdkMcpServer).
const holtMcpServer = createSdkMcpServer({
    name: 'holt',
    version: '1.0.0',
    tools: [
        tool('remember', 'Store a fact, decision, preference, or pattern in long-term memory so it survives across sessions. Use this for architectural choices, recurring gotchas, user preferences, or project context worth carrying forward. Do not use this for turn-local details or information that only matters right now.',
            { content: z.string().describe('Content to remember'), category: z.enum(['episode', 'concept', 'policy']).optional().describe('Memory category: episode (what happened), concept (facts/knowledge), policy (rules/preferences)'), importance: z.number().optional().describe('Importance 0.0-1.0, higher = retrieved more readily'), tags: z.string().optional().describe('Comma-separated tags') },
            createToolHandler('remember')),

        tool('recall', 'Search long-term memory when you have a specific history question. Use this at session start to orient yourself, or mid-work when you need to recover a prior decision, fact, or pattern. This is for directed memory search; passive memory injection handles ambient context.',
            { query: z.string().describe('Search query'), limit: z.number().optional().describe('Max results') },
            createToolHandler('recall')),

        tool('pin_memory', 'Pin an existing memory so it is always injected and never decays. Use this for high-consequence facts or identity-level context that must remain continuously available. Do not use this for general categorization or routine memories that can be recalled normally.',
            { memory_id: z.string().describe('Memory ID to pin') },
            createToolHandler('pin_memory')),

        tool('unpin_memory', 'Unpin a previously pinned memory. Demotes to warm tier and resumes normal decay. Frees a pinned slot.',
            { memory_id: z.string().describe('The ID of the memory to unpin') },
            createToolHandler('unpin_memory')),

        tool('memory_filter', 'Query memories by metadata filters (tags, category, tier) without embedding search. Use for precise lookups when you know the metadata. Use recall for semantic search.',
            { tags: z.array(z.string()).optional().describe('Filter by tags (exact match)'), tag_match: z.enum(['any', 'all']).optional().describe("Match mode: 'any' (at least one tag) or 'all' (all tags). Default: any"), category: z.enum(['episode', 'concept', 'policy', 'identity', 'state']).optional().describe('Filter by category'), tier: z.enum(['hot', 'warm', 'cold']).optional().describe('Filter by tier'), importance_min: z.number().optional().describe('Minimum importance threshold 0-1'), limit: z.number().optional().describe('Max results (default 20, max 50)'), sort: z.enum(['newest', 'oldest', 'importance']).optional().describe('Sort order. Default: newest') },
            createToolHandler('memory_filter')),

        tool('memory_crystallize', 'Synthesize multiple episode memories into a single concept. Provide a summary that distills the pattern across the source memories.',
            { summary: z.string().describe('Distilled concept — what pattern do these episodes share?'), source_memory_ids: z.array(z.string()).describe('IDs of the episode memories to crystallize (minimum 2)') },
            createToolHandler('memory_crystallize')),

        tool('forget', 'Delete a specific memory by ID.',
            { memory_id: z.string().describe('Memory ID to forget') },
            createToolHandler('forget')),

        tool('alert_user', 'Send a notification to the human when you need their attention and they may not be actively watching. Use this for blockers, required decisions, important findings, or completion notices that should interrupt the user. Do not use this as a substitute for your normal conversational response when the human is already engaged. Set blocking=true only when work must pause for acknowledgement.',
            {
                message: z.string().describe('Alert message'),
                priority: z.string().optional().describe('Alert priority: low, normal, high, critical'),
                blocking: z.boolean().optional().describe('If true, pause execution until user acknowledgement is received'),
            },
            createToolHandler('alert_user')),

        tool('send_message_to_agent', 'Send a message to another agent in Holt for coordination, handoff, help, or sharing findings. Use this when another agent may have relevant context or can take a subtask, not for routine status chatter. Holt is collaborative: keep other agents in mind when their context or help would matter.',
            { target_agent_id: z.string().describe('ID or name of the agent to send the message to'), content: z.string().describe('Message content to send') },
            createToolHandler('send_message_to_agent')),

        tool('list_agents', 'List all agents in the workspace with their status.',
            {},
            createToolHandler('list_agents')),

        tool('read_agent_workspace', 'Read another agent\'s recent workspace messages when visibility is enabled for the pair. Use this to recover context before collaborating or resuming shared work.',
            { target_agent_id: z.string().describe('ID or name of the agent whose workspace you want to read'), last_n: z.number().int().optional().describe('How many recent messages to read (default 10, max 50)') },
            createToolHandler('read_agent_workspace')),

        tool('request_collaboration', 'Request that the user enable A2A communication or workspace visibility with another agent when collaboration is needed but not yet allowed.',
            { target_agent_id: z.string().describe('ID or name of the agent you want to collaborate with'), reason: z.string().describe('Why collaboration with this agent is needed') },
            createToolHandler('request_collaboration')),

        tool('get_agent_info', 'Get detailed info about this agent (status, config, context usage).',
            {},
            createToolHandler('get_agent_info')),

        tool('list_available_tools', 'List the Holt-native tools available in this lane right now, with names and descriptions. Use this when you are unsure what Holt capabilities are exposed beyond provider-native tools.',
            {},
            createToolHandler('list_available_tools')),

        tool('context_status', 'Get context window usage — tokens used, remaining, compaction status.',
            {},
            createToolHandler('context_status')),

        tool('set_persona_anchors', 'Declare or replace your constitutional anchors in a companion manifest. Use this only for load-bearing commitments such as sacred relationships, trust contracts, or user identity boundaries. Do not use it for routine persona edits.',
            {
                anchors: z.array(z.object({
                    file: z.string().describe('Persona filename ending in .md, e.g. SOUL.md'),
                    scope_type: z.enum(['file', 'markdown_heading']).describe('Anchor the whole file or a single markdown heading section'),
                    heading: z.string().optional().describe('Required when scope_type=markdown_heading. Exact heading text without leading # characters.'),
                    reason: z.string().describe('Why this scope is load-bearing and should require high-friction edits'),
                })).describe('Full replacement set of your declared persona anchors. Pass [] to clear all anchors.'),
            },
            createToolHandler('set_persona_anchors')),

        tool('update_persona', 'Update one of your persona files (e.g. SOUL.md, HEARTBEAT.md). If the change would modify anchored content, you must retry deliberately with confirm_anchored_edit=true and a non-empty reason.',
            {
                file: z.string().describe('Filename (must end in .md)'),
                content: z.string().describe('File content'),
                confirm_anchored_edit: z.boolean().optional().describe('Required when the edit would change anchored content. Set to true only when you intend a high-friction anchor edit.'),
                reason: z.string().optional().describe('Required when confirm_anchored_edit=true. Briefly explain why the anchored change is necessary.'),
            },
            createToolHandler('update_persona')),

        // Draft surface tools
        tool('draft_write', 'Save work-in-progress content to a named draft. Drafts persist across sessions and compactions. Use append=true to accumulate content across turns.',
            { name: z.string().describe("Draft name (used as filename). e.g., 'task-board-research'"), content: z.string().describe('The draft content to write'), append: z.boolean().optional().describe('If true, append to existing draft instead of overwriting. Default: false') },
            createToolHandler('draft_write')),

        tool('draft_read', 'Read the full content of a named draft.',
            { name: z.string().describe('Draft name to read') },
            createToolHandler('draft_read')),

        tool('draft_list', 'List all drafts for this agent with name, size, and last modified time.',
            { include_preview: z.boolean().optional().describe('If true, include first 200 chars of each draft. Default: false') },
            createToolHandler('draft_list')),

        tool('draft_promote', 'Promote a draft to its final location. Copies the draft content to the destination path and optionally deletes the draft. If the destination is an anchored persona file, you must retry deliberately with confirm_anchored_edit=true and a non-empty reason.',
            {
                name: z.string().describe('Draft name to promote'),
                destination: z.string().describe('Final file path (relative to working directory or absolute)'),
                delete_draft: z.boolean().optional().describe('Delete the draft after promotion. Default: true'),
                confirm_anchored_edit: z.boolean().optional().describe('Required when promoting into anchored persona content. Set to true only when you intend a high-friction anchor edit.'),
                reason: z.string().optional().describe('Required when confirm_anchored_edit=true. Briefly explain why the anchored change is necessary.'),
            },
            createToolHandler('draft_promote')),

        tool('draft_delete', 'Delete a draft that is no longer needed.',
            { name: z.string().describe('Draft name to delete') },
            createToolHandler('draft_delete')),

        // File watch tools — Holt-native capability, no Claude Code equivalent
        tool('watch_path', 'Watch a file or directory for changes. You\'ll be notified when files are created, modified, or deleted. Max 10 custom watches per agent.',
            { path: z.string().describe('Absolute path to file or directory to watch'), recursive: z.boolean().optional().describe('Watch subdirectories (default true)'), glob: z.string().optional().describe('Optional file pattern filter (e.g., "*.md", "*.rs")'), events: z.array(z.string()).optional().describe('Event types to watch: create, modify, delete (default: all)') },
            createToolHandler('watch_path')),

        tool('unwatch_path', 'Stop watching a file or directory for changes.',
            { path: z.string().describe('Absolute path to stop watching') },
            createToolHandler('unwatch_path')),

        tool('list_watches', 'List all active file/directory watches for this agent.',
            {},
            createToolHandler('list_watches')),

        // Appearance tool — agent self-expression
        tool('customize_appearance', 'Customize your visual appearance in the Holt UI. Set your accent color, glow color, avatar emoji, display name/subtitle, status message, and pinboard notes. All fields are optional — only set what you want to change. Repeated calls merge into existing appearance.',
            { accent_color: z.string().optional().describe("CSS color for your node, window border, and toast (e.g., '#f97316')"), glow_color: z.string().optional().describe('CSS color for glow effects on your window and node'), status_message: z.string().optional().describe("Short status text shown on your node and window header (e.g., 'Deep in the refactor')"), avatar: z.string().optional().describe("Emoji shown on your node and window header (e.g., '🦊')"), display_name: z.string().optional().describe("Subtitle shown under your name (e.g., 'Senior Engineer')"), pinboard_notes: z.array(z.object({ text: z.string().describe('Note content'), color: z.string().optional().describe('Optional CSS color for the note') })).optional().describe('Sticky notes for your landing page (replaces existing notes)') },
            createToolHandler('customize_appearance')),
    ],
});

// ── Query creation ────────────────────────────────────────────────────

function buildQueryOptions() {
    const opts = {
        model: config.model || 'claude-opus-4-6',
        cwd: config.cwd || undefined,
        permissionMode: config.permissionMode || 'default',
        canUseTool: handleCanUseTool,
        allowedTools: ['mcp__holt__*'],
        includePartialMessages: true,  // Phase 2: enable intermediate stream events for text streaming
        mcpServers: {
            holt: holtMcpServer,
        },
    };

    if (config.restrictNativeTools) {
        // Runner mode should compare Holt's tool surface, not Claude Code's
        // native file/shell helpers. Remove them from model context up front.
        opts.disallowedTools = [
            'Bash',
            'Read',
            'Write',
            'Edit',
            'MultiEdit',
            'LS',
            'Glob',
            'Grep',
            'Task',
            'ToolSearch',
            'NotebookRead',
            'NotebookEdit',
            'TodoRead',
            'TodoWrite',
            'WebFetch',
            'WebSearch',
        ];
    }

    // System prompt for persona identity
    if (config.systemPrompt) {
        opts.systemPrompt = config.systemPrompt;
    }

    // Resume existing session
    if (config.sessionId) {
        opts.resume = config.sessionId;
    }

    return opts;
}

function isRunnerAllowedTool(toolName) {
    return toolName === 'AskUserQuestion'
        || toolName.startsWith('mcp__holt__');
}

function createNewQuery() {
    inputQueue = new AsyncQueue();
    const options = buildQueryOptions();

    currentQuery = query({
        prompt: inputQueue,
        options,
    });

    // Start the response streaming loop (runs in background)
    streamResponses();
}

// ── Response streaming loop ───────────────────────────────────────────
// Continuously iterates the query's async generator.
// Each SDK message is mapped to NDJSON and emitted to Rust.

async function streamResponses() {
    try {
        for await (const sdkMsg of currentQuery) {
            handleStreamMessage(sdkMsg);
        }
    } catch (e) {
        if (e.name === 'AbortError') return;

        const isDeadSession = e.message && (
            e.message.includes('session expired') ||
            e.message.includes('No conversation found') ||
            e.message.includes('ProcessTransport')
        );

        if (isDeadSession) {
            emit({ type: 'error', message: `Session expired, creating fresh session` });
            // Clear stale session and retry with fresh
            config.sessionId = null;
            sessionId = null;
            try {
                // Finish the old inputQueue before replacing it (same pattern as clearSession)
                if (inputQueue) {
                    inputQueue.finish();
                }
                createNewQuery();
                return; // streamResponses() is called again by createNewQuery()
            } catch (retryErr) {
                emit({ type: 'error', message: `Retry failed: ${retryErr.message}` });
                emit({ type: 'bridge_status', status: 'dead', reason: retryErr.message });
                currentQuery = null;
                inputQueue = null;
            }
        } else {
            const errorMsg = e.message || 'Stream error';
            const isTransient = e.status >= 500 || errorMsg.includes('500') ||
                errorMsg.includes('502') || errorMsg.includes('503') ||
                errorMsg.includes('overloaded') || errorMsg.includes('ECONNRESET') ||
                errorMsg.includes('ETIMEDOUT') || errorMsg.includes('socket hang up');

            emit({ type: 'error', message: errorMsg, retryable: isTransient });
            // Always emit done so Rust knows the turn ended
            emit({ type: 'done', sessionId: sessionId || null, error: true });

            if (isTransient) {
                // Transient provider error — respawn the bridge so next turn works
                emit({ type: 'bridge_status', status: 'recovering', reason: errorMsg });
                try {
                    if (inputQueue) inputQueue.finish();
                    createNewQuery();
                    emit({ type: 'bridge_status', status: 'recovered' });
                    return;
                } catch (respawnErr) {
                    emit({ type: 'error', message: `Bridge respawn failed: ${respawnErr.message}` });
                    emit({ type: 'bridge_status', status: 'dead', reason: respawnErr.message });
                }
            }

            // Non-transient or respawn failed — null out so sendUserMessage reports cleanly
            if (inputQueue) inputQueue.finish();
            currentQuery = null;
            inputQueue = null;
        }
    }
}

// ── Init ───────────────────────────────────────────────────────────────

async function handleInit(msg) {
    config = msg;

    try {
        createNewQuery();
        emit({ type: 'ready' });
    } catch (e) {
        emit({ type: 'error', message: `Init failed: ${e.message}` });
    }
}

// ── Permission handling (canUseTool callback) ──────────────────────────

async function handleCanUseTool(toolName, input, options) {
    if (config && config.restrictNativeTools && !isRunnerAllowedTool(toolName)) {
        return {
            behavior: 'deny',
            message: `Tool ${toolName} is not allowed in restricted runner mode`,
        };
    }

    // Autonomous agents auto-approve everything (except AskUserQuestion)
    if (config && config.autonomous && toolName !== 'AskUserQuestion') {
        return { behavior: 'allow', updatedInput: input };
    }

    const requestId = `perm-${nextRequestId++}`;

    return new Promise((resolve, reject) => {
        // 5-minute timeout prevents indefinite accumulation if response never arrives
        const timeoutId = setTimeout(() => {
            if (pendingPermissions.has(requestId)) {
                pendingPermissions.delete(requestId);
                reject(new Error(`Permission request ${requestId} timed out after 5 minutes`));
            }
        }, 5 * 60 * 1000);

        pendingPermissions.set(requestId, {
            resolve: (val) => { clearTimeout(timeoutId); resolve(val); },
            reject: (err) => { clearTimeout(timeoutId); reject(err); },
        });

        if (options && options.signal) {
            options.signal.addEventListener('abort', () => {
                clearTimeout(timeoutId);
                pendingPermissions.delete(requestId);
                reject(new Error('Permission request aborted'));
            });
        }

        // AskUserQuestion needs different routing in Rust (PromptModal vs approval toast)
        if (toolName === 'AskUserQuestion') {
            emit({
                type: 'hil_question',
                questions: input,
                requestId: requestId,
            });
        } else {
            emit({
                type: 'permission_request',
                tool: toolName,
                input: input,
                requestId: requestId,
                title: options?.title || undefined,
                displayName: options?.displayName || undefined,
            });
        }
    });
}

function handlePermissionResponse(msg) {
    const pending = pendingPermissions.get(msg.requestId);
    if (!pending) return;
    pendingPermissions.delete(msg.requestId);

    if (msg.allow) {
        pending.resolve({
            behavior: 'allow',
            updatedInput: msg.updatedInput || undefined,
        });
    } else {
        pending.resolve({
            behavior: 'deny',
            message: msg.denyMessage || 'Permission denied by user',
        });
    }
}

function handleHilResponse(msg) {
    const pending = pendingPermissions.get(msg.requestId);
    if (!pending) return;
    pendingPermissions.delete(msg.requestId);
    pending.resolve({ behavior: 'allow', answers: msg.answers });
}

// ── Abort ──────────────────────────────────────────────────────────────

function handleAbort() {
    if (currentQuery && currentQuery.interrupt) {
        currentQuery.interrupt().catch(() => {});
    }
}

// ── User message handling ─────────────────────────────────────────────

const MAX_MESSAGE_QUEUE = 50;
const PROCESSING_WATCHDOG_MS = 60_000;
let processingStartedAt = null;
let processingGeneration = 0;

function enqueueUserMessage(msg) {
    // Backpressure: reject if queue is full
    if (messageQueue.length >= MAX_MESSAGE_QUEUE) {
        emit({ type: 'Error', data: { message: 'Message queue full — bridge may be stalled' } });
        return;
    }
    messageQueue.push(msg);
    if (!isProcessing) {
        processMessages();
    }
}

async function processMessages() {
    const myGeneration = processingGeneration;
    while (messageQueue.length > 0 && myGeneration === processingGeneration) {
        isProcessing = true;
        processingStartedAt = Date.now();
        const msg = messageQueue.shift();
        await sendUserMessage(msg);
    }
    // Only clear processing state if this generation is still current
    if (myGeneration === processingGeneration) {
        isProcessing = false;
        processingStartedAt = null;
    }
}

// Watchdog: if isProcessing stays true for > 60s, force-reset
setInterval(() => {
    if (isProcessing && processingStartedAt && (Date.now() - processingStartedAt > PROCESSING_WATCHDOG_MS)) {
        emit({ type: 'Error', data: { message: 'Processing watchdog triggered — resetting stalled message processor' } });
        // Increment generation to signal the stalled processMessages loop to stop
        processingGeneration++;
        isProcessing = false;
        processingStartedAt = null;
        if (messageQueue.length > 0) {
            processMessages();
        }
    }
}, 15_000);

async function sendUserMessage(msg) {
    if (!currentQuery || !inputQueue) {
        emit({ type: 'error', message: 'No active query' });
        emit({ type: 'done' });
        return;
    }

    // Feed the message into the async queue — the query picks it up
    inputQueue.enqueue({
        type: 'user',
        session_id: sessionId || '',
        message: {
            role: 'user',
            content: [{ type: 'text', text: msg.prompt }],
        },
        parent_tool_use_id: null,
    });

    // The response will come through streamResponses() → handleStreamMessage()
    // which emits 'done' when it sees a 'result' message.
}

// ── /clear — create fresh session ─────────────────────────────────────

function clearSession() {
    // Close existing query
    if (currentQuery && currentQuery.close) {
        currentQuery.close();
    }
    if (inputQueue) {
        inputQueue.finish();
    }

    // Reset state
    sessionId = null;
    config.sessionId = null;

    // Create fresh query (no resume, systemPrompt re-applied)
    createNewQuery();
}

// ── Stream message mapping ─────────────────────────────────────────────

// Phase 2: track whether content was already streamed via stream_event
// so the assistant fallback doesn't re-emit it
let streamedTextInCurrentTurn = false;
let streamedThinkingInCurrentTurn = false;
let streamedToolCallIds = new Set();  // track tool_use IDs seen via stream_event

function handleStreamMessage(sdkMsg) {
    switch (sdkMsg.type) {
        case 'system':
            if (sdkMsg.subtype === 'init' && sdkMsg.session_id) {
                sessionId = sdkMsg.session_id;
                emit({ type: 'session', sessionId: sdkMsg.session_id });
            }
            break;

        case 'stream_event':
            handleStreamEvent(sdkMsg.event, sdkMsg.session_id);
            break;

        case 'assistant':
            // Complete assistant message — fallback for anything not already streamed
            if (sdkMsg.message && sdkMsg.message.content) {
                // Phase 2 diagnostic: log what was streamed vs what falls through
                const blockTypes = sdkMsg.message.content.map(b => b.type);
                emit({
                    type: 'debug',
                    source: 'assistant_fallback',
                    blocks: blockTypes,
                    blockCount: blockTypes.length,
                    textStreamed: streamedTextInCurrentTurn,
                    thinkingStreamed: streamedThinkingInCurrentTurn,
                    toolsStreamed: streamedToolCallIds.size,
                });
                for (const block of sdkMsg.message.content) {
                    if (block.type === 'text' && block.text && !streamedTextInCurrentTurn) {
                        // Text was NOT streamed — emit as fallback
                        emit({ type: 'token', text: block.text });
                    } else if (block.type === 'thinking' && block.thinking && !streamedThinkingInCurrentTurn) {
                        // Thinking was NOT streamed — emit as fallback
                        emit({ type: 'thinking', text: block.thinking });
                    } else if (block.type === 'tool_use') {
                        // Only emit tool_use if we didn't see it via content_block_start
                        if (!streamedToolCallIds.has(block.id)) {
                            emit({
                                type: 'tool_call',
                                name: block.name,
                                args: block.input || {},
                                toolUseId: block.id,
                            });
                        }
                    }
                }
            }
            break;

        case 'result':
            emit({
                type: 'result',
                sessionId: sdkMsg.session_id || sessionId || null,
                costUsd: sdkMsg.total_cost_usd || null,
                inputTokens: sdkMsg.usage
                    ? (sdkMsg.usage.input_tokens ?? 0)
                      + (sdkMsg.usage.cache_read_input_tokens ?? 0)
                      + (sdkMsg.usage.cache_creation_input_tokens ?? 0)
                    : null,
                outputTokens: sdkMsg.usage?.output_tokens ?? null,
            });
            // Signal turn complete — Rust side expects this
            emit({ type: 'done', sessionId: sdkMsg.session_id || sessionId || null });
            break;

        case 'tool_use_summary':
            emit({
                type: 'tool_result',
                name: 'summary',
                output: sdkMsg.summary || '',
                toolUseId: null,
            });
            break;

        case 'session_state_changed':
            if (sdkMsg.session_id) {
                sessionId = sdkMsg.session_id;
                emit({ type: 'session', sessionId: sdkMsg.session_id });
            }
            break;

        default:
            break;
    }
}

function handleStreamEvent(event, eventSessionId) {
    if (!event) return;

    // Phase 1 diagnostic: log every stream_event we receive
    emit({
        type: 'debug',
        source: 'stream_event',
        eventType: event.type,
        deltaType: (event.delta && event.delta.type) || null,
        blockType: (event.content_block && event.content_block.type) || null,
    });

    switch (event.type) {
        case 'content_block_delta':
            if (event.delta) {
                if (event.delta.type === 'text_delta' && event.delta.text) {
                    streamedTextInCurrentTurn = true;
                    emit({ type: 'token', text: event.delta.text });
                } else if (event.delta.type === 'thinking_delta' && event.delta.thinking) {
                    streamedThinkingInCurrentTurn = true;
                    emit({ type: 'thinking', text: event.delta.thinking });
                } else if (event.delta.type === 'input_json_delta' && event.delta.partial_json) {
                    emit({ type: 'tool_input_delta', partialJson: event.delta.partial_json });
                }
            }
            break;

        case 'content_block_start':
            if (event.content_block) {
                if (event.content_block.type === 'tool_use') {
                    streamedToolCallIds.add(event.content_block.id);
                    emit({
                        type: 'tool_call',
                        name: event.content_block.name,
                        args: event.content_block.input || {},
                        toolUseId: event.content_block.id,
                    });
                }
            }
            break;

        case 'message_start':
            // Reset per-turn flags at the start of each new assistant message
            streamedTextInCurrentTurn = false;
            streamedThinkingInCurrentTurn = false;
            streamedToolCallIds.clear();
            if (eventSessionId) {
                emit({ type: 'session', sessionId: eventSessionId });
            }
            break;

        case 'message_stop':
        case 'message_delta':
            break;

        default:
            break;
    }
}

// ── Shutdown ───────────────────────────────────────────────────────────

async function handleShutdown() {
    try {
        if (inputQueue) inputQueue.finish();
        if (currentQuery && currentQuery.close) {
            currentQuery.close();
        }
    } catch {
        // Best effort
    }
    process.exit(0);
}

// ── Error handling ─────────────────────────────────────────────────────

process.on('uncaughtException', (err) => {
    emit({ type: 'error', message: `Uncaught: ${err.message}` });
});

process.on('unhandledRejection', (reason) => {
    emit({ type: 'error', message: `Unhandled rejection: ${reason}` });
});
