import { describe, it, expect } from 'vitest';
import { buildQueryOptions, isRunnerAllowedTool, permissionResult, resultEvents, RESTRICTED_NATIVE_TOOLS, DEFAULT_MODEL, MCP_SERVER_NAME } from './core.js';

const deps = { holtMcpServer: { name: 'holt' }, canUseTool: () => {} };

describe('buildQueryOptions', () => {
  it('isolates SDK agents from filesystem settings', () => {
    // SDK 0.3 loads the user's Claude Code settings, hooks and CLAUDE.md when this is omitted.
    expect(buildQueryOptions({}, deps).settingSources).toEqual([]);
  });

  it('wires only the Holt MCP server', () => {
    const o = buildQueryOptions({}, deps);
    expect(MCP_SERVER_NAME).toBe('holt');
    expect(Object.keys(o.mcpServers)).toEqual(['holt']);
    expect(o.mcpServers.holt).toBe(deps.holtMcpServer);
    expect(o.allowedTools).toEqual(['mcp__holt__*']);
  });

  it('keeps the permission mode explicit and falls back to the default model', () => {
    const o = buildQueryOptions({}, deps);
    expect(o.permissionMode).toBe('default');
    expect(o.model).toBe(DEFAULT_MODEL);
    expect(buildQueryOptions({ model: 'some-model', permissionMode: 'plan' }, deps)).toMatchObject({ model: 'some-model', permissionMode: 'plan' });
  });

  it('never offers the question widget, which has no card on this lane', () => {
    expect(buildQueryOptions({}, deps).disallowedTools).toEqual(['AskUserQuestion']);
  });

  it('removes every native tool in restricted runner mode, by the names the current CLI uses', () => {
    const o = buildQueryOptions({ restrictNativeTools: true }, deps);
    for (const t of ['Bash', 'Read', 'Write', 'Edit', 'Glob', 'Grep', 'Agent', 'TaskCreate', 'TodoWrite', 'WebFetch', 'WebSearch', 'Skill', 'Workflow', 'CronCreate', 'Monitor', 'AskUserQuestion']) {
      expect(o.disallowedTools).toContain(t);
    }
    expect(o.disallowedTools).toEqual(RESTRICTED_NATIVE_TOOLS);
    for (const gone of ['MultiEdit', 'LS', 'NotebookRead', 'TodoRead']) {
      expect(o.disallowedTools).not.toContain(gone);
    }
  });

  it('passes the persona and resume id through only when present', () => {
    expect(buildQueryOptions({}, deps)).not.toHaveProperty('systemPrompt');
    expect(buildQueryOptions({}, deps)).not.toHaveProperty('resume');
    expect(buildQueryOptions({ systemPrompt: 'P', sessionId: 'S' }, deps)).toMatchObject({ systemPrompt: 'P', resume: 'S' });
  });
});

describe('isRunnerAllowedTool', () => {
  it('allows only Holt MCP tools in runner mode', () => {
    expect(isRunnerAllowedTool('mcp__holt__recall')).toBe(true);
    expect(isRunnerAllowedTool('Bash')).toBe(false);
    expect(isRunnerAllowedTool('AskUserQuestion')).toBe(false);
    expect(isRunnerAllowedTool('mcp__other__recall')).toBe(false);
  });
});

describe('permissionResult', () => {
  it('always returns the input on allow (an allow without it was rejected before SDK 0.3.207)', () => {
    expect(permissionResult({ allow: true }, { command: 'ls' })).toEqual({ behavior: 'allow', updatedInput: { command: 'ls' } });
    expect(permissionResult({ allow: true, updatedInput: { command: 'pwd' } }, { command: 'ls' })).toEqual({ behavior: 'allow', updatedInput: { command: 'pwd' } });
  });

  it('denies with a message', () => {
    expect(permissionResult({ allow: false }, {})).toEqual({ behavior: 'deny', message: 'Permission denied by user' });
    expect(permissionResult({ allow: false, denyMessage: 'no' }, {})).toEqual({ behavior: 'deny', message: 'no' });
  });
});

describe('resultEvents', () => {
  const usage = { input_tokens: 1, cache_read_input_tokens: 2, cache_creation_input_tokens: 3, output_tokens: 4 };

  it('reports a successful turn', () => {
    const ev = resultEvents({ type: 'result', subtype: 'success', session_id: 's1', total_cost_usd: 0.5, usage }, null);
    expect(ev).toEqual([
      { type: 'result', sessionId: 's1', costUsd: 0.5, inputTokens: 6, outputTokens: 4 },
      { type: 'done', sessionId: 's1' },
    ]);
  });

  it('reports an error result as an error, not a quiet success', () => {
    const ev = resultEvents({ type: 'result', subtype: 'error_max_turns', is_error: true, session_id: 's1', errors: ['hit the turn limit'] }, null);
    expect(ev[0]).toMatchObject({ type: 'result', sessionId: 's1' });
    expect(ev[1]).toEqual({ type: 'error', message: 'Turn ended with error_max_turns: hit the turn limit', retryable: false });
    expect(ev[2]).toEqual({ type: 'done', sessionId: 's1', error: true });
  });

  it('falls back to the tracked session id', () => {
    expect(resultEvents({ type: 'result', subtype: 'success' }, 'tracked')[1]).toEqual({ type: 'done', sessionId: 'tracked' });
  });
});
