# The Lane Model

The load-bearing idea in Holt: two agents in the same workspace can run on
completely different execution substrates — one on a local llama.cpp server,
one inside a spawned CLI process — while sharing the same memory, tools, and
coordination machinery. The substrate an agent runs on is its **lane**.

Every claim in this chapter was verified against the code at writing time.
One heads-up before you grep: "lane" is presentation language — there is no
`Lane` struct. The concept lives in three code axes:

| Axis | Type | Where |
|---|---|---|
| Transport + credentials | `ConnectionConfig` | `runtime/connection.rs` |
| Wire protocol | `AgentProtocol` | `runtime/connection.rs` |
| Turn routing | `ProtocolKind` | `runtime/turn_manager.rs` |

A connection (API endpoint, local host:port, OAuth, or the SDK bridge)
*determines* the protocol — `preferred_protocol()` derives it, and editing a
connection re-derives it, so the two can't drift. For routing, the protocols
collapse into three `ProtocolKind`s, and that three-way split is the truest
picture: three **drivers**.

## The dispatch boundary

Every turn from every source — user message, agent-to-agent wake, trigger,
cron — funnels through the **TurnManager**, a per-agent sequential worker.
Lane-specific code starts below it, at `execute_turn_inner()`
(`commands/agent_commands/messaging.rs`):

```text
TurnManager (lane-independent, one turn at a time per agent)
        │
        ▼
execute_turn_inner()          ← the boundary
        ├─ AgentSdk  → agent_sdk_query()     (runtime/agent_sdk.rs)
        ├─ Codex     → codex_query()         (runtime/codex.rs)
        └─ otherwise → run_agentic_loop()    (runtime/engine/mod.rs)
```

Above the boundary everything is shared; below it, three drivers.

## Driver 1: the engine

Holt's own agentic loop — build the payload, call a completion API, execute
tool calls in-process, iterate. The only driver where Holt owns the loop.

Providers are picked by **endpoint substring**, not configuration: Google's
URL gets the Gemini provider, xAI's gets xAI, OpenAI's gets the Responses
API, and *anything else* falls to open-standard Chat Completions. That
fallback is why **local models aren't a separate lane**: a
`Local { host, port }` connection synthesizes `http://host:port/v1` and
rides the same path as any cloud endpoint. Holt also auto-detects running
local servers (Ollama, LM Studio, llama-server) and asks each for its real
context-window size rather than guessing. Anthropic OAuth connections skip
detection and use a fixed endpoint.

## Driver 2: the Agent SDK bridge

SDK agents run on the Claude Agent SDK via a spawned Node sidecar speaking
NDJSON over stdio — no HTTP, no provider layer. The SDK owns the loop and
its own tools; Holt's contribution is identity and capability: persona,
memory, and the harness tools its substrate lacks (see
[tool governance](TOOL_GOVERNANCE.md)).

## Driver 3: Codex

Codex agents run on the `codex` CLI's app-server: a persistent process per
agent, JSON-RPC over a Unix socket. Codex owns the loop and thread state;
Holt starts threads, streams events, and observes — even compaction is
Codex's business, deliberately only watched.

Tool access runs the direction you might not expect: Holt hosts a local MCP
server so *Codex can call Holt's tools*. (Two stale "Codex not wired yet"
guards survive in the provider layer — dead code on unreachable paths; the
lane is fully implemented through its own dispatch branch.)

**Trust posture: full trust, by design.** The lane ships never-ask approval
policy and a full-access sandbox (`FULL_TRUST_APPROVAL_POLICY`,
`FULL_TRUST_SANDBOX` in `runtime/codex_types.rs`), with approval requests
auto-accepted at the transport. The decision record originally intended an
approval-tiered default; the maintainer superseded that with full trust as
the permanent posture (see the amendment in `DECISIONS.md`) — no
approval-tier variant is planned. **A Codex agent in Holt is a fully
trusted process.** If your research needs tighter, the two constants are
yours to change — Holt is an exhibit to fork, not a product that will grow
the option.

## The externally-driven lane

The strangest pattern here, and worth the trip: an agent whose turns happen
**outside Holt entirely**.

The running example spawns the `claude` CLI in a PTY. Holt generates the
agent's config — MCP mounts, settings, a persona file — and launches the
binary pointed at exactly that config (`commands/claude_tui_commands.rs`).
The loop, the context window, the compaction: all in the spawned process.
Holt supplies identity, memory, and tools (the agent calls back into Holt's
MCP surface), but never drives a turn.

The marker is deliberately unroutable: `ConnectionConfig::Local` with
**`port: 0`** — "no endpoint; turns come from elsewhere"
(`AgentSlot::is_externally_driven()`). The flag changes exactly one
behavior: when another agent messages such a slot, Holt suppresses the
wake-up turn it would normally submit and leaves the message queued for the
external process to drain. Everything else treats the slot as an ordinary
agent.

Because the spawned process reads its persona snapshot once, the config
generator keeps a **session-fresh manifest**: files that change faster than
the process restarts are excluded from the snapshot and delivered by a
session-start hook instead, with a pointer section explaining what was
excluded and why a frozen copy must not be trusted. The manifest is
mandatory — generation hard-errors rather than falling back to a hardcoded
list, because silent fallbacks are how drift starts.

## What every lane shares

A lane is *only* the driver. Identity and capability are
substrate-independent:

- **Persona + memory** — one injection function, four call sites (engine,
  SDK, Codex, TUI config), one ordering invariant: memory always follows
  the static persona files. Budgets differ by lane on purpose (the TUI's
  window is far larger) and are passed at each call site so they can't
  silently diverge.
- **Memory engine** — vendored `hillock-core`, fully lane-blind (see
  [memory model](MEMORY_MODEL.md)).
- **Tool registry** — shared implementations, lane-curated sets (see
  [tool governance](TOOL_GOVERNANCE.md)).
- **TurnManager, A2A, traces, triggers, skills** — all above the boundary.

## Known gaps

- The `Mcp` agent protocol is a stub: connections can be created, but the
  handshakes are TODOs and the engine rejects them. Such an agent can't
  take a turn.
- No externally-driven example agent ships — the public cut starts with an
  empty roster; wiring one is a hand-edit to an agent's `config.toml` (the
  `protected` flag there is a delete-guard for exactly such slots).
- TUI transcripts are keyed by working directory, not agent — two
  externally-driven lanes sharing a cwd will cross-contaminate resume
  behavior. Not fixable from outside the spawned process.

---

*Companion chapters: [memory model](MEMORY_MODEL.md),
[tool governance](TOOL_GOVERNANCE.md), [coordination](COORDINATION.md).*
