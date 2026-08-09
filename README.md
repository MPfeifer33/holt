# Holt

**A local-first research harness for persistent, multi-agent orchestration —
with memory, personas, tool governance, and human-in-the-loop attention as
first-class architecture.**

Holt is the public cut of a private system that has run a real multi-agent
household for months. The private system's history, residents, and data stay
home; the mechanisms — the parts worth studying — are here, runnable, with
fresh history and honest documentation.

Holt is a public research snapshot of a private system that has run
multi-agent orchestration for months; it is not a maintained application or
framework.

A holt is a den. The code is what lives in it.

## Why It Exists

Most agent frameworks are session-shaped: spin up, do a task, evaporate.
The interesting problems start when agents *persist* — when they keep
identity, memory, working relationships, and obligations across weeks of
real use. Holt is the harness those problems were worked in:

- What does an agent lane need so it survives restarts, compactions, and
  model swaps without losing itself?
- Where does memory actually belong — in weights, in context, or in an
  engine beside the model — and what does retrieval owe the truth?
- What changes when several agents with different substrates share one
  house, one memory discipline, and one human?

Holt doesn't answer these with a paper. It answers with a codebase you can
run, instrument, and disagree with.

## What's Inside

- **Persistent agent lanes** — named agents with their own connection,
  protocol, working directory, tools, limits, and conversation history.
  Local models, cloud APIs, MCP servers, and an Agent SDK sidecar are all
  first-class connection types; capability truth derives from the
  connection, not from session luck.
- **Archetype personas** — a registry of composable bases and
  specializations that generate deterministic cognitive profiles at agent
  creation. Personas are configuration, not hardcoded residents.
- **Memory (Hillock)** — a vendored SQLite memory engine with explicit
  save/recall, importance scoring, pinning, episode→concept crystallization,
  and cross-space result provenance. The familiar hot/warm/cold **tiering is
  a harness-layer policy, not an engine feature**: the injection layer (in
  this repo) composes pinning, importance, and relevance into tiered context
  delivery over the engine's flat primitives. **Isolation is
  geometric:** each private memory space's embeddings are rotated by a
  matrix in SO(d) derived from key material (Argon2id → QR decomposition),
  computed at startup and never persisted — vectors at rest are unreadable
  across spaces without the space's key; shared spaces use encrypted random
  seeds, the commons rides identity. Within a space, retrieval is
  brute-force cosine — deliberately simple, honestly documented, fast at
  household scale. The engine ships **empty**: architecture public,
  memories private.
- **Tool surface with governance** — filesystem/shell/web tools behind
  approval tiers, per-turn and per-session budgets, circuit breakers for
  repetitive tool loops, and structured receipts on results.
- **Traces** — SQLite logging of runtime events for after-the-fact
  archaeology of what an agent actually did.
- **Externally-driven lanes** — agents whose turns originate outside the
  app (a terminal session, another harness) while the app still carries
  their identity, persona injection, and coordination surface.
- **Checkpoint/restore, session persistence, context compaction** — the
  unglamorous machinery that makes "persistent" true.

## What It Is Not

- Not a product, not supported, not stable. Expect rough edges; they're
  part of the exhibit.
- Not cloud-anything. Local-first; your keys live in your OS keychain;
  nothing phones home.
- Not the private system. Fresh history, no residents, no data. Some
  internal code keeps the predecessor's naming by deliberate decision —
  see `docs/DECISIONS.md`.

## Quick Start

```bash
npm install
npm run tauri dev      # development app
npm run tauri build    # production build (frontend + sidecar + binary)

# backend tests
cargo test --manifest-path app/src-tauri/Cargo.toml
```

Create an agent in the UI, point it at a local OpenAI-compatible endpoint
(llama.cpp, Ollama, LM Studio) or a cloud key, and start working. The
`docs/` directory grows chapters as they're written — the decision record and
initial public release notes are already there.

## Documentation

- `PROJECT.md` — current state, layout, conventions
- `docs/DECISIONS.md` — every consequential call in the public cut, with
  rationale
- `docs/RELEASE_NOTES.md` — initial public-cut status, validation snapshot,
  known limitations, and release notes
- Architecture chapters: [lane model](docs/LANE_MODEL.md) ·
  [memory model](docs/MEMORY_MODEL.md) ·
  [tool governance](docs/TOOL_GOVERNANCE.md) ·
  [coordination](docs/COORDINATION.md)

## Status

Research harness, cut 2026-08. Backend tests green (859). The public cut
is live and the architecture chapters are written. Holt is an exhibit —
what you see is what it is; rough edges and all are part of the display.

## License and Attribution

PolyForm Small Business 1.0.0 — see `LICENSE.md` and `NOTICE.md`.
Personal use: free, unconditionally. Small-business use (under 100 people
and $1M revenue): free. Beyond that — including selling products built on
this — requires a commercial license. Credit is required and travels with
every copy.

Copyright HearthByte (Mark Pfeifer).
