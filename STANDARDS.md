# STANDARDS.md

Code standards and preferred patterns for the Holt codebase. All agents and contributors must follow these when writing or modifying code.

**Adopted:** 2026-03-25 (v0.3.0)
**Scope:** All code in `app/src-tauri/src/` (Rust) and `app/src/` (Svelte/TypeScript).

## Contents

- [Transition Clause](#transition-clause)
- [1. Rust Conventions](#1-rust-conventions)
- [2. Error Handling](#2-error-handling)
- [3. Architectural Principles](#3-architectural-principles)
- [4. Frontend Conventions](#4-frontend-conventions)
- [5. What's Good — Preserve These](#5-whats-good--preserve-these)
- [6. Established Patterns (Post-V0.3)](#6-established-patterns-post-v03)
- [7. Anti-Patterns](#7-anti-patterns)
- [8. Testing & Verification](#8-testing--verification)

## Transition Clause

**Updated:** 2026-05-03

The original engine refactor eliminated `streaming.rs` and `claude_sdk.rs`. Remaining debt targets:

| Module | Lines | Primary Debt |
|--------|-------|--------------|
| `runtime/engine/mod.rs` | ~1284 | God-module tendencies, mixed responsibilities |
| `runtime/context.rs` | ~1275 | Oversized, multiple concerns conflated |
| `runtime/skills.rs` | ~1312 | Oversized, needs decomposition |
| `runtime/engine/compaction.rs` | ~792 | Growing; monitor for split |
| `runtime/engine/tool_execution.rs` | ~738 | Acceptable if cohesive, but verify |

These are being addressed by the **full refactor pass** (Step 2.5 in the project plan). The rules below apply to:
- All **new** modules and files created after adoption.
- All **modified** modules — if you're making substantial changes (not a targeted bugfix), bring the module closer to compliance.
- **Targeted bugfixes** (under ~20 lines) in pre-refactor modules are exempt. Add a `// TODO(standards):` comment noting the violation if you spot one while fixing.

---

## 1. Rust Conventions

### Module Responsibility

Every module has a **single responsibility** and **one reason to change**. The metric is **cohesion**, not size.

Decision rules:
- **The sentence test:** Can you describe the module's purpose in one sentence without "and" connecting *unrelated* ideas? "Manages PTY lifecycle and tracks session state" — fine, facets of one thing. "Manages PTY lifecycle and handles keyboard shortcuts" — two modules.
- **The change-blast test:** Does modifying one feature in this file force you to understand or risk breaking a different feature in the same file? If yes, split by responsibility.
- **The scatter test:** Would splitting actually help? If the resulting pieces constantly import each other's internals and can't function independently, you've just scattered context. Splitting should create modules with clean, narrow API surfaces between them.
- **Size is a signal, not the rule.** A 1200-line parser that handles one grammar is fine. A 400-line file that manages config, validates input, and dispatches events is three modules regardless of line count. **1000+ lines deserves a second look** to confirm it's genuinely one concern — but if it is, it stays.
- If the change you're making is unrelated to the module's core purpose, create a new module. Don't bolt unrelated features onto existing files just because they're "close enough."

### Naming

- Types: `PascalCase`. Functions/methods: `snake_case`. Constants: `SCREAMING_SNAKE_CASE`.
- Module files named after the primary type or concept they contain (e.g., `approval.rs` contains `ApprovalManager`).
- No generic names: no `utils.rs`, `helpers.rs`, `common.rs`. Name by what it does.
- Tauri commands: `{domain}_{verb}_{noun}` pattern (e.g., `agent_get_status`, `warroom_create`, `plugin_list`).

### Import Ordering

Group imports in this order, separated by blank lines:
1. `std` library
2. External crates
3. Internal crate modules (`crate::`, `super::`)

### Visibility

- Default to private. Only `pub` what's needed by other modules.
- `pub(crate)` for internal cross-module sharing. `pub` only for the Tauri command surface.

### Struct Design

- Prefer small, focused structs.
- **8-field threshold:** If a struct has more than 8 fields, decompose it into sub-structs unless the fields are all logically inseparable parts of a single concept. If you keep it flat, document why.
- Context/service bags should group fields by **lifecycle and domain**. Fields that are always passed together and used together belong in one group. If a function only uses 3 of 8 fields from a context struct, the struct is too broad for that function.

### Dependencies & Async

- `Arc<T>` for shared ownership across async boundaries.
- `Arc<RwLock<T>>` for shared mutable state. Prefer `RwLock` over `Mutex` for read-heavy access.
- **`tauri::async_runtime::spawn`** for general async tasks — Tauri 2.x wraps tokio and manages its own runtime lifecycle.
- **`tokio::task::spawn_blocking`** is permitted for genuinely blocking I/O (PTY reads, filesystem ops) that would starve the async executor. Wrap in a tokio JoinHandle and track for shutdown.
- **`tokio::spawn`** is acceptable inside `spawn_blocking` closures or for fire-and-forget background work that doesn't need Tauri's lifecycle management (e.g., one-shot metric reporting). Prefer `tauri::async_runtime::spawn` when in doubt.

### Concurrency Discipline

- **Hold locks for minimum duration.** Clone data out, drop the lock, then work.
- **Never hold two write locks simultaneously** without documenting the acquisition order. With many `Arc<RwLock<T>>` fields in AppState, undocumented lock ordering leads to deadlocks.
- Prefer `read()` over `write()` when you don't need mutation.

### Cancellation Discipline

- All new async loops must check `CancellationToken` at each iteration boundary.
- All spawned tasks must be tracked for clean shutdown.
- If a task holds resources (VFL locks, file handles, shell sessions), it must release them on cancellation.

### Unsafe

No `unsafe` blocks without a `// SAFETY:` comment explaining the invariant being upheld.

**`unsafe impl Sync` pattern:** When wrapping crate types that are `Send` but not `Sync` (e.g., `portable_pty::MasterPty`), place all non-Sync fields behind `Arc<Mutex<T>>` or `Arc<std::sync::Mutex<T>>`, then add `unsafe impl Sync` with a `// Safety:` comment listing which fields are wrapped and why. This is the established pattern (see `terminal/session.rs`).

---

## 2. Error Handling

### At the Tauri Command Boundary

`Result<T, String>` — forced by Tauri IPC, don't fight it. But the string must carry context: what was being attempted, what failed, why. Not just the leaf error message.

### At Module Boundaries (`runtime/`, `tools/`)

- Typed errors via `thiserror`. Each module defines its own error enum when it has distinct failure modes.
- **Distinguish user-actionable from internal errors.** Use separate error variants or types. User-actionable: missing API key, bad config, agent not found — things the user can fix. Internal: parser failure, unexpected state, deserialization bug — things that need debugging. The Tauri command boundary is responsible for mapping internal errors to user-facing strings.

### Within a Module

- Pragmatic. `anyhow::Result` or `?` with conversion is fine for internal helpers.
- Prefer `?` propagation over manual match/unwrap chains.

### Rules

- **Never silently swallow errors.** If caught and discarded, log with `tracing::warn!` or `tracing::error!` and document why.
- **Log the full chain internally, surface the actionable part to the user.** The user sees "API key not configured for provider X." The trace log gets the full error chain with HTTP status, endpoint URL, and response body.
- **`expect()` is acceptable in synchronous initialization** where failure is truly unrecoverable and the message clearly explains the failure (e.g., `expect("trace DB must be accessible")`). No `unwrap()` or `expect()` in async code paths.

---

## 3. Architectural Principles

- **Prefer composition over inheritance.** If inheritance (deep trait hierarchies, blanket impls, trait inheritance chains) is genuinely the right call for stability, performance, or functionality — use it, but it must be documented. Inline comment explaining why inheritance was chosen over composition, and a note in the module's doc comment. The "why" is mandatory, not optional.

- **Trait objects for polymorphism.** When multiple implementations exist (providers, tools, transports), define a trait. Don't use enums with match arms that grow forever. Note: existing traits like `Tool` may gain new methods (e.g., `undo()`, `requires_approval()`) as the architecture evolves — extending a working trait's surface is not a violation.

- **Service grouping over god structs.** Related fields belong in a sub-struct with its own methods. Group by lifecycle and domain, not by convenience.

- **Modules own their types.** The struct definition and its core logic live in the same module. If you're reaching across module boundaries to manipulate another module's internals, the API surface is wrong.

- **Don't abstract prematurely.** Three similar functions is fine. Extract when a fourth instance arrives AND the shared logic is identical in structure, not just similar in concept. A wrong abstraction is harder to undo than duplication.

---

## 4. Frontend Conventions

### Svelte

- **Svelte 5 runes only.** `$state`, `$derived`, `$effect`. Never the older `$:` reactive syntax.
- **No `$effect` at module level.** Use explicit function calls.
- **`<script module>`** not `<script context="module">`.
- **Components follow single-responsibility.** Same cohesion rules as Rust modules — the sentence test, change-blast test, and scatter test all apply. Size is a signal, not the rule.

### Stores & IPC

- **Stores live in `lib/stores/`** as `.svelte.ts` files. One store per domain (agents, canvas, UI, etc.).
- **Reactive utilities** (shortcut bindings, orchestration logic) live in their own `lib/{domain}/` directory as `.svelte.ts` or `.ts` files. Not everything reactive is a "store" — coordination modules, state machines, and handlers get their own homes.
- **IPC calls go through `lib/tauri/commands.ts`** for standard CRUD operations. Components never call `invoke()` directly for these.
- **High-frequency IPC exception:** Components managing real-time I/O (terminal write, streaming) may call `invoke()` directly when the typed wrapper adds latency or indirection without safety benefit. Document with a `// Direct invoke: high-frequency I/O` comment.
- **CamelCase in `invoke()` calls.** Tauri 2 auto-converts to snake_case for Rust. Don't manually snake_case.

### TypeScript

- Avoid `any` types. Use proper interfaces or `unknown` with type guards when the type isn't known.
- IPC return types should match Rust serialization. Shared type definitions between Rust and TypeScript are preferred.
- `npm run check` (Svelte type checking) is the required frontend verification step until a linter is configured.

---

## 5. What's Good — Preserve These

Patterns that work. Do not refactor these away without a design spec justifying the change.

1. **`Tool` trait with `execute()`** — clean, extensible, well-organized by domain in `tools/`.
2. **Registry pattern** — `ToolRegistry`, `SkillRegistry`, `WarRoomRegistry`.
3. **Sandbox isolation** — `workspace_root` vs `working_directory` distinction.
4. **Plugin architecture** — Native + MCP transport abstraction, sidecar bridge.
5. **Approval tiers** — 4-tier system (Auto, NotifyUnlessVeto, RequireApproval, Blocked) with bash pattern escalation.
6. **VFL (Virtual File Locking)** — queue-based, explicit `release().await`. Not Drop-only.
7. **Svelte 5 rune stores** — the store pattern in `lib/stores/`.
8. **Trace system** — SQLite WAL, per-type truncation.
9. **Tools directory structure** — organized by domain (`filesystem/`, `shell/`, `a2a/`, etc.).
10. **Persona system** — `SOUL.md`/`USER.md` loading with 32k char limit.
11. **Connection auto-detection** — port scanning for local models on startup.
12. **Atomic session persistence** — JSON writes with temp-file-then-rename. Crash-safe.
13. **A2A message drain pattern** — drain at turn boundary, not mid-stream. Prevents race conditions. (Note: the pattern is solid; A2A integration coverage across non-SDK agents is still expanding.)
14. **Config round-trip serialization** — TOML save/reload without data loss, including plugin registry HashMap. Verified safe as of v0.3.0. If changing config structure, re-run serialization round-trip tests and verify.

---

## 6. Established Patterns (Post-V0.3)

Patterns introduced after the original standards adoption that are now load-bearing. Follow these when building similar features.

### Floating Window Pattern

Draggable, resizable floating panels (terminal, A2A panel, agent chat) follow this structure:

- **Self-managed position:** `x`, `y`, `width`, `height` as `$state()`. No CSS `transform: translate()` for positioning — use `style="left:{x}px; top:{y}px"`.
- **Pointer-capture drag:** Header element captures pointer on `pointerdown`, tracks offset, releases on `pointerup`. Never use mousedown/mousemove (pointer events handle touch and pen too).
- **8-edge resize handles:** Absolute-positioned divs at edges and corners. Each handle captures its own pointer for independent resize tracking.
- **ResizeObserver:** Content that needs to reflow on resize (xterm, code editors) uses a ResizeObserver on the container, debounced with `requestAnimationFrame`.
- **Focus zone:** Set `data-zone="{name}"` on the root element for keyboard shortcut routing.
- **Minimize/restore:** Minimized state renders a pill in a dock area. Restore brings back the full window at its last position.

Reference: `lib/canvas/TerminalWindow.svelte`

### Thread-Based I/O with Event Emission

For blocking I/O that doesn't fit async (PTY, serial ports, blocking FFI):

1. Spawn a real `std::thread` for the blocking read loop.
2. Emit data to the frontend via `app_handle.emit("event-name", payload)`.
3. Track liveness with `Arc<AtomicBool>` — set to `false` on EOF/error, check in write paths.
4. Wrap the thread in `tokio::task::spawn_blocking` + `JoinHandle` for cancellation.
5. Emit an exit event on loop termination so the frontend can update UI state.

Reference: `terminal/session.rs`

### Keyboard Shortcut System

- **Bindings config:** `HashMap<String, String>` in app config. Key = action name, value = binding string (e.g., `"Ctrl+Shift+T"`).
- **Focus zones:** Handler detects which zone has focus via `data-zone` attributes on ancestors of `document.activeElement`.
- **Terminal passthrough:** A defined set of key combos (Ctrl+C, Ctrl+D, etc.) are never intercepted when the terminal zone has focus.
- **Chord sequences:** Two-step shortcuts (e.g., Ctrl+K then 1) use a state machine with a 1500ms timeout between keystrokes.
- **Conflict detection:** Config exposes `find_conflicts()` to prevent duplicate bindings at registration time.

Reference: `lib/shortcuts/`

### Gzip Log Rotation

For persistent structured logging (A2A messages, audit trails):

- **Daily files:** `{prefix}-YYYY-MM-DD.log.gz` in `~/.local/share/holt/logs/{domain}/`.
- **Thread-safe writer:** `Arc<Mutex<LoggerInner>>` where `LoggerInner` holds the current `GzEncoder` and date.
- **Date rollover:** Check current date on each write. If day changed, finish the current encoder and open a new file.
- **Graceful on failure:** Log rotation failures are logged via `tracing::warn!` but don't crash the caller.

Reference: `runtime/a2a_logger.rs`

### Memory-Bridge Integration

- **Access pattern:** Go through `app_state.get_memory_engine()` for bridge operations. Never construct bridge clients ad-hoc.
- **Failure tolerance:** Bridge calls in non-critical paths (metrics, compaction reporting) use fire-and-forget with logged errors. Bridge calls in critical paths (recall, remember) propagate errors to the caller.
- **No blocking on bridge in hot paths:** Turn execution, streaming, and tool dispatch must not await bridge operations synchronously. Use spawn for async reporting.

---

## 7. Anti-Patterns

### Module Discipline

1. **No god modules.** If adding to a file and the change is unrelated to its core purpose, create a new module. Detection heuristic: if describing the module requires "and" to connect *unrelated* responsibilities, it needs to be split. Two facets of the same responsibility (e.g., "approval evaluation and bash pattern escalation") can stay together.

2. **No feature work on a module already violating standards.** Split first, then add. This prevents the next `engine.rs`. Exception: targeted bugfixes under ~20 lines (see Transition Clause).

### Error Discipline

3. **No silent error swallowing.** Every caught-and-discarded error gets a log line and a comment explaining why.

### Runtime Discipline

4. **Prefer `tauri::async_runtime::spawn`** for lifecycle-managed tasks. `tokio::spawn` / `spawn_blocking` permitted for blocking I/O and fire-and-forget background work (see Dependencies & Async).
5. **No `Drop` as primary cleanup.** Async resources get explicit cleanup calls. `Drop` is the safety net, not the plan.
6. **No blanket `unwrap()` or `expect()` in async code.** Use `?` or handle the error. (See Section 2 for `expect()` in sync init.)

### Code Hygiene

7. **No premature abstraction.** Don't extract until a fourth instance proves the pattern and the shared logic is structurally identical.
8. **No backwards-compatibility shims.** Unused code gets deleted. No `_old` suffixes, no re-exports of moved types, no `// removed` comments. Example: if `EngineContext` moves from `engine.rs` to `execution/types.rs`, delete the old location and update all callers. Don't leave a re-export behind.
9. **No `clone()` to dodge the borrow checker.** Cloning `Arc` is fine — that's what it's for. Cloning large structs (`Vec<AgentSlot>`, `AppConfig`) solely to avoid restructuring lifetimes means the function signature is wrong. Fix the signature. If you genuinely need a snapshot (e.g., before/after comparison, sending across a channel), clone with a comment explaining why.
10. **No raw `invoke()` in components** for standard operations. Go through `lib/tauri/commands.ts`. Exception: high-frequency I/O (see Stores & IPC).

### When Standards Conflict with Velocity

If following a standard requires disproportionate effort relative to the change (e.g., a 3-file refactor for a 5-line fix), document the deviation with a `// TODO(standards): [which rule] [why deferred]` comment and move on. Do not silently ignore standards. These TODOs are addressed during the next refactor pass touching that module.

---

## 8. Testing & Verification

- **`cargo test` must pass before any commit.** No exceptions.
- **`cargo clippy` — no new warnings.** Run before commits alongside tests.
- **`cargo check` for fast iteration.** Full test + clippy before committing.
- **Config structs need round-trip serialization tests.** New config blocks get a serialize-then-deserialize-and-verify test.
- **Approval configs in test fixtures must include all fields.** Specifically `require_approval_timeout_seconds`.
- **Manual smoke test after engine/streaming/SDK changes:** Start app, create agent, send message, verify streaming, verify tool execution.
- **Frontend:** `npm run check` is the required verification step until a linter is configured post-refactor.
