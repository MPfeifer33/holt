# Coordination

Holt has no orchestrator. No planner agent, no task router, no DAG deciding
who works next. Coordination is two humbler things standing beside each
agent: a **mailbox** with a permission graph, and a **turn queue** that lets
one agent do one thing at a time. Everything else in this chapter is the
engineering that keeps a household of autonomous agents from melting down —
and the dampers turn out to be the interesting part.

## One turn at a time

The `TurnManager` keeps a worker per agent: a FIFO queue and a single loop
that runs turns sequentially for that agent while different agents run
fully concurrently. Every turn source — user messages, peer messages,
crons, file watches — submits through it, and the queue discipline spans
all three lane drivers identically (in-house engine, Agent SDK, Codex —
see the [lane model](LANE_MODEL.md)).

The failure posture is "degrade, don't die": a panicking turn is caught and
becomes an error result instead of stranding the queue; a dead worker is
replaced on next use; a *full* queue rejects new turns rather than
blocking. (A `priority` field rides along on every request but nothing
enforces it yet — the queue is honest FIFO.)

One carve-out: subagents — short-lived workers an agent can spawn for a
scoped task — run as bare async tasks outside the turn discipline. They
inherit the parent's connection, tools, and sandbox, but deliberately
**not** its personality: subagents get a neutral prompt, with the reason
recorded in-code — preventing sycophancy inheritance from
high-agreeableness parents. Results queue up and are delivered into the
parent's next turn. Depth is one; subagents cannot spawn subagents.

## Peer messages are system context, not fake users

Agent-to-agent messaging is direct and flat: sender, target, content. The
delivery detail most frameworks get wrong is one Holt refuses on purpose:
**a peer message is never injected as a user message.** It arrives as a
system-prompt section, clearly attributed — because an earlier version
injected it as `role: user` and models confused their colleagues with their
human. The engine drains the inbox at each loop iteration, so mail arriving
mid-turn still lands in that turn.

Who may message whom is a directed permission graph — pairs are opened by
team structure or by hand, and an agent facing a closed pair is told which
tool to call to ask the human to open it. Workspace visibility (reading
another agent's recent activity) is a separate grant on the same graph.

## The dampers

Autonomous agents that can message, wake, and schedule each other are a
feedback system, and Holt's coordination layer is mostly damping,
accumulated the honest way:

- **Send-time wake only.** A message wakes an idle target once, at send. An
  earlier design re-woke agents whenever they finished a turn with mail
  pending — and produced infinite A↔B reply loops. The removal, and the
  full reasoning, are preserved as a comment in the engine.
- **Wake cooldown** — five seconds per agent, so a burst of messages is one
  wake, not ten.
- **A per-turn send guard** — an agent can't message the same target twice
  in one turn.
- **Skip-if-busy** — scheduled jobs firing at a busy agent record a skip
  instead of queueing pile-ups.
- **Self-write suppression** — an agent editing a file it watches doesn't
  wake itself in an echo loop.
- **Bounded everything** — inbox overflow flows to a dead-letter queue
  instead of erroring the sender; nothing unbounded, every overflow with a
  defined behavior.

The one agent that's never woken: an externally-driven lane
(`port: 0` — see the lane model). Waking it would drain its mailbox into a
turn with no model behind it, silently destroying the messages, so its mail
waits for the external process to collect.

## Agents schedule their own future

Crons and file watches are one job registry with two trigger types, and
agents hold the tools to create them — an agent can schedule its own
check-in, watch a directory, and be woken by the change. Watches are
debounced, filtered by glob and event type, and covered by the self-write
suppression above. Every job tracks its fire and skip counts; a config kill
switch stops the scheduler wholesale.

## You can see what happened, and undo some of it

Every consequential event — messages, tool calls with their execution
receipts, memory operations, subagent lifecycles, peer traffic — lands in a
SQLite trace store, truncated per-type. A2A additionally streams to its own
compressed log.

Checkpoints snapshot an agent's *conversation* (messages, system prompt,
working directory, active-subagent status) — not the filesystem, not other
agents. Restore is non-destructive and self-announcing: it auto-saves a
pre-restore checkpoint first, then tells the agent, in-conversation, that
its history was rewound and where the undo lives. An agent always knows
when its past has been edited.

---

*Companion chapters: [lane model](LANE_MODEL.md),
[memory model](MEMORY_MODEL.md), [tool governance](TOOL_GOVERNANCE.md).*
