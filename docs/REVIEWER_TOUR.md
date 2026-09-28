# Reviewer's Tour: Holt in fifteen minutes

*For an engineer deciding whether the author is someone to work with. Six stops in
order, with a sentence on why each one matters. Every claim below can be checked in
the file it names.*

**0. Run the tests first (two minutes).**

```bash
cargo test --manifest-path app/src-tauri/Cargo.toml
```

865 pass. If one named `test_hash_deterministic` fails on a fresh clone, it needs
the embedding model cached locally; everything else is hermetic.

**1. `docs/LANE_MODEL.md` (four minutes).** What an "agent lane" is: a named agent
with its own connection, protocol, working directory, tools, limits, and history.
It also explains why what an agent can do is derived from its connection rather
than from whatever the last session happened to leave behind. Read this first and
the rest of the code has a shape.

**2. `app/src-tauri/src/runtime/turn_manager.rs` (three minutes).** Every turn an
agent takes goes through one per-agent worker: serialized, prioritized, cancellable
by token. Read the tests at the bottom first; they say what the file guarantees.

**3. `app/src-tauri/src/tools/registry.rs` (two minutes).** One tool implementation
and three transports (native, MCP, SDK sidecar). The registry removes tools by lane
and sandbox level rather than adding them. The tests named `*_curates_out_*` are
the invariant.

**4. `app/src-tauri/src/memory/injection.rs` (two minutes).** The memory engine
underneath is flat. Tiering is a policy composed here from pinning, importance, and
relevance. The engine never claims to be more than it is.

**5. `app/src-tauri/src/runtime/approval.rs` (two minutes).** Approval tiers and the
human-in-the-loop path: a tool call parks on a oneshot until a person answers the
card. See `docs/TOOL_GOVERNANCE.md` for the tiers.

**6. `docs/DECISIONS.md` and `docs/RELEASE_NOTES.md` (two minutes).** What was
decided and why, plus a dated security entry (2026-09-28) for three findings from a
full audit of the private predecessor. Each was fixed with a test that was watched
failing first.

**If you have five more minutes:** `docs/MEMORY_MODEL.md` for the geometric
isolation of private memory spaces (embeddings rotated by a key-derived matrix in
SO(d), never persisted), which is the part people ask about.

**What this repo is not:** a maintained framework or a product. It is a research
snapshot of a system that has run a real multi-agent setup for months, cut public
with fresh history. Rough edges are documented rather than hidden.
