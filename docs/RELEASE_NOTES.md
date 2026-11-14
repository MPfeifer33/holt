# Holt Release Notes

## 2026-11-13 — Staleness pass: current provider APIs, API-key Anthropic lane, memory fixes

Holt had not been touched since 2026-10-01, and the things around it moved:
provider request shapes, model line-ups, the Claude Agent SDK and the Codex
CLI. The predecessor went through the same review two days earlier; the fixes
that have a counterpart in Holt's tree are ported here. None of this has been
run against a live provider, a live Codex or SDK turn, or the desktop app. It
is covered by unit tests only.

### Anthropic lane

- **API key instead of subscription sign-in.** The Messages provider sends
  `x-api-key` and the agent's own system prompt. An API connection whose
  endpoint is `api.anthropic.com` routes to it. The earlier path, which used a
  Claude subscription token and identified itself as the Claude Code CLI, is
  removed together with the module that read and refreshed that token.
  Anthropic's terms reserve subscription sign-in for its own applications. A
  Claude subscription still works through the Agent SDK lane, which runs the
  unmodified Claude Code binary.
- **Current request shape.** Models from 4.6 on think adaptively and reject a
  manual `budget_tokens`; 4.5 and earlier keep it. With thinking off, adaptive
  models are asked for low effort.
- **Thinking blocks are replayed correctly.** Each block is stored with its
  own signature and sent back exactly as received, including empty signed and
  redacted blocks. Unsigned thinking text is never replayed.
- **A refusal is an error**, not an empty reply. Stream bytes are decoded only
  once a line is complete, so a multi-byte character split across chunks
  survives.

### Models

- One table (`runtime/context/model_sizes.rs`) holds the default Claude model,
  the short aliases and current context sizes for Claude, OpenAI, xAI and
  Google. The settings structs and the model picker ask it.
- The Gemini default moves to `gemini-3.8-flash`; the 2.5 line shuts down
  2026-12-01. Grok 4.3 and later are recognised as always-reasoning.

### Agent SDK lane

- SDK `0.3.322`. The user's own Claude Code settings and hooks no longer load
  into SDK agents (`settingSources: []`), and only Holt's own MCP tools are
  pre-allowed.
- **Permission answers now reach the bridge.** A native-tool request raises
  the approval card and the answer is written back; before, it sat until the
  bridge's five-minute rejection.
- An error result from the SDK is reported as an error.

### Codex lane

- A turn that fails with an error code Holt does not know is reported as a
  failure instead of a successful empty reply.
- The `on-failure` approval policy is gone from the schema and from
  `/approval`. `/effort` accepts the current range. Rollback falls back to
  `thread/revert` on servers that no longer know `thread/rollback`.

### Memory

- `hillock-core` 0.0.2: importance takes part in ranking and only observations
  decay; a deliberate recall reinforces a memory, automatic reads do not; an
  exact repeat of a saved memory is folded in instead of duplicated; a long
  write is one transaction; a deleted memory gives up its pin.
- Automatically captured rows are left out of default `recall` and can be
  asked for with `include_captured`. `recall` can fetch one memory whole by
  id. `remember`, `pin`, `unpin`, `forget` and `memory_crystallize` report what
  the engine did; `memory_crystallize` writes to the calling agent's own space.
- Maintenance runs on a schedule (`[memory] nightly_interval_hours`, 24 by
  default, 0 turns it off). It used to run only when an agent asked.
- Not ported: the predecessor's summary tree, note extraction and supersession
  cascade. They live in a newer engine that Holt does not vendor.

### Dependencies

- npm advisories in shipped packages: 11 → 0. Rust patch updates for
  advisories in rustls, rustls-webpki, webpki-roots, h2, quinn, tokio and
  others.
- Still open: `libsqlite3-sys` (needs a `rusqlite` major bump) and `quick-xml`
  (transitive). Dev-only npm advisories under Vite 6 remain.

### Validation

- Rust backend tests: 942 passed, 0 failed, 2 ignored. Memory engine: 126.
- Frontend: 25 unit tests, `npm run check` 0 errors, sidecar bundle rebuilt,
  `npm audit --omit=dev` 0 vulnerabilities.

## 2026-09-28 — Security fixes from the predecessor's audit

Three findings from a full audit of the private predecessor (2026-09-28) apply
to Holt's tree verbatim and are fixed here the same day, each with a test that
was watched failing first.

- **`start_process` ran outside the sandbox.** bwrap wrapped only the PTY
  bash session; the background-process tool spawned `shell -c` directly with
  a caller-chosen working directory. It now builds a `LaunchPlan`
  (`tools/shell/process.rs`): sandboxed agents run inside the same bwrap
  namespace as their shell and only inside the workspace, restricted agents
  are refused, unrestricted agents go to the shell as before. The sandbox
  comes from the agent's live PTY config or, failing that, its `config.toml`.
- **A corrupt per-agent `config.toml` silently granted the default tool set at
  `unrestricted`.** `agent_config::tools_and_sandbox_from` fails closed:
  missing file → defaults, unparseable file → error. Wired at the
  send-message boundary and `list_available_tools`.
- **OAuth refresh rewrote the credentials file with the umask mode (0644).**
  The temp file is now created 0600 and fsynced before the rename.

Not ported: the predecessor's runtime moved to a new lane in September 2026;
its step-loop fixes have no counterpart in Holt's engine.

## 2026-08-08 — Initial public research cut

Holt is now public as a fresh-history research harness for persistent,
local-first multi-agent orchestration.

Holt is a public research snapshot of a private system that has run multi-agent
orchestration for months; it is not a maintained application or framework.
This initial cut is intended for study, experimentation, and extension, not as
a supported product release.

### What is included

- Fresh public repository history.
- Public-safe source tree with no bundled private runtime data.
- Vendored, scrubbed `hillock-core` memory engine snapshot under
  `crates/hillock-core/`.
- Holt-branded package, application, config, keychain, and sidecar identities.
- Public decision record in `docs/DECISIONS.md`.
- Local-first architecture for agent lanes, personas, tool governance,
  memory integration, traces, checkpoints, and externally-driven lanes.

### What is intentionally not included

- Private repository history.
- Private conversations, memories, traces, or generated coordination state.
- Resident agent persona files or household-specific runtime data.
- Credentials, API keys, tokens, local password hints, or machine-local
  configuration.
- Private project diary material and internal cutover audit details.

### Validation snapshot

The public cut was validated locally before publication with:

- Rust formatting for the Tauri backend package.
- Rust backend tests: 859 passed, 0 failed, 2 ignored.
- Frontend diagnostics: `npm run check` passed with 0 errors and 0 warnings.
- Frontend production build: `npm run build` passed.
- Frontend dependency audit: `npm audit --audit-level=low` reported 0
  vulnerabilities.
- Agent SDK sidecar bundle check: `npm run build:sidecar` passed.
- Vendored Hillock validation: 87 tests passed.
- Public scrub scan for private provenance terms, resident identities,
  machine-local paths, and credential patterns.
- Agent-tool suite pass over the public repo using Latch, Probe, Atlas,
  Sentinel, Switchboard, and Witness.

### Known limitations

- No full `npm run tauri dev` browser smoke is claimed for the public-cut tree.
- No portable public build outside the maintainer machine is claimed yet.
- Architecture chapters are still in progress and will land as ordinary
  documentation commits.
- Sentinel risk confidence is expected to be low at first because the public
  repository starts with intentionally fresh, thin history.

### Notes for readers

Holt is the public, identity-free cut of a private predecessor system. The
private system's data, history, and residents remain private; Holt publishes the
mechanisms that are useful for research.
