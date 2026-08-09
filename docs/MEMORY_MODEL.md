# The Memory Model

Holt agents persist. Sessions end, context windows fill and compact, processes
restart — and the agent is still supposed to be *someone* afterward. The memory
engine is how. It's called Hillock, it's vendored in-tree at
`crates/hillock-core`, and it ships empty: the mechanism is public, the
memories it once held are not.

## Embedded, on purpose

There is no vector database, no memory server, no port. Hillock is a Rust
crate compiled into the app: SQLite as the source of truth, an in-memory
vector index rebuilt at startup, and retrieval by brute-force cosine scan.
That last part is deliberate — at the scale of one household of agents
(thousands of memories, not billions), a linear scan over 768-dimensional
vectors is sub-millisecond, and the code names its own escape hatch: a
`VectorIndex` trait as the boundary where an ANN index would slot in *when
scale demands it*. It hasn't. (You'll find "HNSW" in comments and even a
function name — naming residue for that future, not a description of the
present.)

The payoff of embedding is that memory can't be *down*. The health check
literally returns `true` with a comment: the embedded engine is always
reachable.

## The interesting part: memory spaces are rotated

Most multi-tenant memory systems isolate agents with a filter — a
`WHERE agent_id = ?` that everyone hopes is never forgotten. Hillock does
something stranger: **every agent's memories live in a rotated coordinate
system.**

Each private space gets a rotation matrix R ∈ SO(768), derived
deterministically: master key + agent id → Argon2id → a BLAKE3 keyed
expansion into a full matrix → QR decomposition → a proper rotation
(orthogonal, determinant +1 — the test suite checks each property). Every
embedding is rotated on the way in; every query is rotated on the way to
retrieval.

Because rotation is orthogonal, cosine similarity *inside* a space is
exactly preserved — retrieval quality costs nothing. But a vector from one
agent's space, compared against another's, is noise under an unrelated
basis. The isolation is geometric: it holds even against the query that
forgot its filter. Matrices are runtime-only, rebuilt from seeds at boot,
never persisted.

Honest edges, so nobody over-claims on our behalf: the seeds themselves are
stored (this is compartmentalization keyed on the master key, not encryption
at rest), and an unconfigured install falls back to a literal
`"default-dev-key"` — set a real key if the isolation matters to you.

## What an agent actually experiences

Eight tools: `remember`, `recall`, `memory_filter`, `pin_memory`,
`unpin_memory`, `forget`, `memory_crystallize`, `memory_cleanup`. A memory
carries content, a category, importance, tags, and provenance; storing one
runs a pipeline — content-hash dedup, embed, rotate, a scan for near-
duplicates and contradictions, chunking for long content. Retrieval is
hybrid (vector plus SQLite FTS5 full-text, fused) with a small recency
weight whose value was chosen against a written table, in the source, of
what each candidate weight would have overturned.

The design bet worth noticing: **memory is not force-fed.** Holt does not
stuff retrieved memories into every turn. An agent's context gets memory
automatically exactly once — at session start, where pinned memories pack
first (budget-exempt, five pins maximum) and the remaining token budget
fills by importance, appended after the agent's persona files, identically
on every lane. After that, remembering is a deliberate act: the agent calls
`recall` the way you'd actually consult your memory, when something needs
looking up. (An earlier per-turn relevance-injection pipeline existed; it
was retired, and the module that housed it now enforces its own absence
with a test.)

The UI shows memories in hot/warm/cold tiers — read that as a *view*, not
state: hot means pinned, cold means low importance. The engine's own comment
is blunter: decay is continuous, no tiers.

## Forgetting is engineered, not hoped for

The lifecycle ops are where the scars show. Importance decays continuously
with age; low-importance memories get pruned; pinned memories are immune to
all of it. The nightly maintenance run *refuses to proceed* if its database
backup fails. Crystallization condenses multiple episodes into one concept
and hides — not deletes — its sources; "superseded" used to mean cleanup and
now means *kept but excluded from recall*, a change made after a
decay-cascade deleted 104 memories in a day. Deletions are soft, cascade-
aware, and audit-logged.

Memories can also cross agents, as a modeled operation rather than a shared
table: sharing un-rotates from the source space and re-rotates into the
target, stamping provenance (which agent, which space, when). A key ring
controls which spaces an agent's recall may search; a shared `commons` space
(identity rotation) is the household bulletin board, and grants are
revocable.

---

*Companion chapters: [lane model](LANE_MODEL.md),
[tool governance](TOOL_GOVERNANCE.md), [coordination](COORDINATION.md).*
