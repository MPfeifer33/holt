# Tool Governance

Holt has one set of tool implementations — fifty-eight of them, from
`read_file` to `remember` to `spawn_subagent` — and **no two lanes see the
same subset, reach them the same way, or pass the same gate.** That
asymmetry is the design, and it's the thing to understand about how Holt
treats tools.

## Curation by subtraction

The registry (`tools/registry.rs`) builds a different tool set per
substrate. An engine-lane agent gets nearly everything: Holt *is* its hands,
so it needs filesystem, shell, web, verification, memory, coordination. An
Agent-SDK or Codex agent gets roughly two-thirds — and the missing third is
exactly filesystem/shell/web, because those substrates already own superior
versions of them. Holt offers a lane only what its substrate lacks: memory,
agent-to-agent messaging, drafts, watches, harness introspection.

The subtraction is a maintained invariant, not a side effect — there are
tests whose entire job is to assert that the SDK and Codex registries do
*not* contain `read_file` or `bash`
(`sdk_registry_curates_out_redundant_coding_tools`).

Within a lane, per-agent config narrows further: capability flags
(filesystem, code execution, web access, A2A), a restricted sandbox level
that strips write/shell tools, and a coordinator mode that trades
`spawn_subagent` away. Subagents can't spawn subagents — depth one,
enforced by construction.

## One implementation, three transports

`RememberTool` executes identically no matter who calls it — what varies is
the road in:

- **Engine agents** call tools natively; the engine loop executes them
  in-process.
- **SDK agents** see Holt as an in-process MCP server: the sidecar bridge
  registers the curated tools with real schemas (`createSdkMcpServer`,
  `mcp__holt__*`), and each call round-trips over stdio into the same Rust
  registry.
- **Codex agents** reach the same registry through an MCP-over-HTTP server
  Holt runs locally for them.

So Holt is an MCP *server* twice over (to the SDK and to Codex) and an MCP
*client* as well — plugins can mount external MCP servers whose tools are
bridged into every lane's registry as if native. The plugin host is a full
extension surface beyond tools: plugins can hook message pre/post-
processing, trace export, and compaction save/restore.

A quieter budget trick, engine-lane only: just ~17 "core" tools ship as
native function schemas in the request; the rest are discoverable through a
catalog tool and described on demand. The tool surface is treated as a
token cost to manage, not a fixed manifest to dump into context.

## Three trust models, deliberately different

- **Engine lane** — the full gate. Every tool call passes a four-tier
  approval check: `Auto`, `NotifyUnlessVeto` (a window to object),
  `RequireApproval` (a blocking ask), `Blocked`. Defaults ship cautious
  where it counts (`delete_file` requires approval; fetching URLs and
  process control are veto-able) and pattern rules can escalate an
  otherwise-auto `bash` call. The timeout asymmetry is the philosophy in
  miniature: an unanswered *veto* proceeds, an unanswered *approval
  request* denies. Denials return to the model as a structured refusal —
  the agent learns "no," the turn survives.
- **SDK lane** — delegates to the SDK's own permission callback; Holt
  answers it (autonomous agents auto-allow everything except questions
  directed at the human).
- **Codex lane** — full trust, by decision: never-ask approval policy, full-
  access sandbox, approval requests auto-accepted at the transport (see
  the lane model chapter and `DECISIONS.md`). A Codex agent in Holt is a
  fully trusted process; the useful configuration, per the maintainer, is
  the shipped one.

Orthogonal to all three: **permission profiles** — named bundles
(`unrestricted` through `restricted`) over thirteen tool families, enforced
at the Codex MCP boundary on *both* `tools/list` and `tools/call`, so a
stale tool list can't be replayed past a profile change.

## Receipts, not claims

Every tool declares an authority class — informational, effectful, or
effectful-verified — and every engine-lane execution emits a receipt into
the trace carrying what *actually* happened: executed or not, verified or
not, blocked and why. The harness records ground truth alongside the
model's narration, which is the difference between auditing an agent and
taking its word for it.

## Skills

Tools are capabilities; **skills** are technique — markdown documents
(instructions, checklists, house patterns) that agents load into context.
They live in a user directory, auto-resolve in the engine lane by trigger
rules (always / keyword / pattern / tool-based) under a token budget, and
are available on demand everywhere via a `read_skill` tool. Three starter
skills ship as seeds. It's the same bet as the memory model: don't force
everything into every prompt — make retrieval cheap and deliberate.

---

*Companion chapters: [lane model](LANE_MODEL.md),
[memory model](MEMORY_MODEL.md), [coordination](COORDINATION.md).*
