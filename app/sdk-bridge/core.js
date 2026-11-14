/**
 * Pure parts of the Agent SDK bridge: what goes to `query()` and how SDK
 * answers map onto the NDJSON the Rust side reads. No I/O, no SDK import,
 * so they can be tested without spawning a CLI.
 */

/** Matches the default in `config/agent_sdk_config.rs`; Rust normally sends the model. */
export const DEFAULT_MODEL = 'claude-opus-5-5';

/** Name of the in-process MCP server that carries Holt's tools. */
export const MCP_SERVER_NAME = 'holt';
const MCP_TOOL_PREFIX = `mcp__${MCP_SERVER_NAME}__`;

/**
 * Claude Code's question widget. Holt has no card that can answer it on
 * this lane, so the model is never offered it and asks in chat instead.
 */
const QUESTION_TOOL = 'AskUserQuestion';

/**
 * Every native tool the current CLI registers, for runner mode, which
 * compares Holt's tool surface and nothing else.
 */
export const RESTRICTED_NATIVE_TOOLS = [
    'Bash',
    'Read',
    'Write',
    'Edit',
    'Glob',
    'Grep',
    'NotebookEdit',
    'Agent',
    'TaskCreate',
    'TaskGet',
    'TaskUpdate',
    'TaskList',
    'TaskStop',
    'TodoWrite',
    'ToolSearch',
    'WebFetch',
    'WebSearch',
    'Skill',
    'Workflow',
    'Monitor',
    'CronCreate',
    'CronDelete',
    'CronList',
    'ScheduleWakeup',
    'EnterPlanMode',
    'ExitPlanMode',
    'EnterWorktree',
    'ExitWorktree',
    QUESTION_TOOL,
];

/**
 * Options for `query()`. `settingSources: []` keeps SDK agents out of the
 * user's own Claude Code settings, project settings, hooks and CLAUDE.md:
 * from SDK 0.3 an omitted value loads all of them.
 */
export function buildQueryOptions(config, { holtMcpServer, canUseTool }) {
    const opts = {
        model: config.model || DEFAULT_MODEL,
        cwd: config.cwd || undefined,
        permissionMode: config.permissionMode || 'default',
        settingSources: [],
        canUseTool,
        allowedTools: [`${MCP_TOOL_PREFIX}*`],
        disallowedTools: config.restrictNativeTools ? RESTRICTED_NATIVE_TOOLS : [QUESTION_TOOL],
        includePartialMessages: true,
        mcpServers: { [MCP_SERVER_NAME]: holtMcpServer },
    };
    if (config.systemPrompt) {
        opts.systemPrompt = config.systemPrompt;
    }
    if (config.sessionId) {
        opts.resume = config.sessionId;
    }
    return opts;
}

export function isRunnerAllowedTool(toolName) {
    return toolName.startsWith(MCP_TOOL_PREFIX);
}

/**
 * The `canUseTool` answer for a `permission_response` from Rust. An allow
 * always carries the input: the tool runs with exactly what was approved.
 */
export function permissionResult(msg, input) {
    if (msg.allow) {
        return { behavior: 'allow', updatedInput: msg.updatedInput || input };
    }
    return { behavior: 'deny', message: msg.denyMessage || 'Permission denied by user' };
}

/**
 * NDJSON events for an SDK `result` message. An error result (turn limit,
 * budget, execution failure) is reported as an error before `done`.
 */
export function resultEvents(sdkMsg, trackedSessionId) {
    const sessionId = sdkMsg.session_id || trackedSessionId || null;
    const usage = sdkMsg.usage;
    const events = [
        {
            type: 'result',
            sessionId,
            costUsd: sdkMsg.total_cost_usd || null,
            inputTokens: usage ? (usage.input_tokens ?? 0) + (usage.cache_read_input_tokens ?? 0) + (usage.cache_creation_input_tokens ?? 0) : null,
            outputTokens: usage?.output_tokens ?? null,
        },
    ];
    const failed = sdkMsg.is_error === true || (typeof sdkMsg.subtype === 'string' && sdkMsg.subtype.startsWith('error'));
    if (failed) {
        const detail = Array.isArray(sdkMsg.errors) && sdkMsg.errors.length > 0 ? `: ${sdkMsg.errors.join('; ')}` : '';
        events.push({ type: 'error', message: `Turn ended with ${sdkMsg.subtype || 'an error'}${detail}`, retryable: false });
        events.push({ type: 'done', sessionId, error: true });
    } else {
        events.push({ type: 'done', sessionId });
    }
    return events;
}
