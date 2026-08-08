# Holt Public Cut Audit

Status: first scrub checkpoint ready
Started: 2026-08-08
Source: private predecessor snapshot; exact provenance is tracked outside this repository.

## Scope

Create a separate, fresh-history Holt repository suitable for eventual public
release as a research harness. The private predecessor repository must stay
untouched.

## Non-Negotiables

- No private git history.
- No credentials, keys, tokens, or local password hints.
- No private session/conversation fixtures.
- No resident agent persona files.
- No household-specific docs or project diary material.
- No hardcoded resident lanes in the final public seed.
- No public release until the maintainer explicitly flips the repo public.

## Completed So Far

- Created isolated working copy for Holt.
- Initialized fresh git repository with no commits.
- Used local Switchboard/Latch coordination for the cut; no generated
  coordination database is retained in the public source tree.
- Excluded obvious generated/private runtime directories during copy:
  - `.git/`
  - `.agent-atlas/`
  - `.agent-sentinel/`
  - `.agent-witness/`
  - `.agent-workspace/`
  - `.claude/`
  - `.superpowers/`
  - `OLD/`
  - frontend/backend build artifacts
  - `node_modules/`
- Quarantined accidental generated/cache artifacts outside the repo:
  - `app/src-tauri/.fastembed_cache/`
  - `.agent-workspace/`
  - `docs/archive/Archive.zip`
- Quarantined house-history docs outside the repo:
  - `CHANGELOG.md`
  - `PROJECT.md`
  - `DIAGNOSIS-*.md`
  - `docs/archive/`
  - `docs/plans/`
  - `docs/mockups/`
  - `docs/specs/PITCH_DECK.md`
- Quarantined resident-lane scripts outside the repo:
  - `scripts/set-sdk-model.sh`
  - `scripts/tui-a2a-inbox.sh`
  - `scripts/tui-a2a-poller.sh`
  - `scripts/test_tui_a2a_inbox.sh`
- Quarantined inherited private/research artifacts outside the repo:
  - `app/src-tauri/context-data-fixes.md`
  - `app/src-tauri/hardcoded-uix-audit.md`
  - inherited private branding assets
  - `spike-tests/`
  - stale root `.agents/`
  - stale root `src/`
- Replaced the inherited README with a Holt public-cut README.
- Removed the private resident-lane auto-bootstrap path from Holt startup.
- Removed the hardcoded resident-lane UI special case.
- Switched app-local runtime namespaces to public Holt naming for:
  - Tauri package/product identity;
  - config directory;
  - keychain service;
  - Hillock memory fallback namespace;
  - bundled Agent SDK MCP bridge.
- Neutralized several resident-specific tests and examples.
- Vendored a scrubbed `hillock-core` snapshot into `crates/hillock-core/` and
  repointed Holt's backend dependency to the in-tree crate.
- Removed predecessor config/keychain migration behavior so Holt starts from
  its own config directory and keychain service only.

Quarantine path:

```text
<outside-repo>/tmp/holt-quarantine-2026-08-08/
```

## Known Blockers Before Public Release

### B1: Resident lane hardcoded in source

Initial examples found:

- `app/src-tauri/src/lib.rs`
  - `bootstrap_nix_agent`
  - `build_nix_tui_slot`
  - `ensure_nix_tui_profile_on_disk`
  - hardcoded resident lane IDs
  - resident system prompt text
- `app/src/routes/+page.svelte`
  - special-case UI behavior for the resident lane
- `app/src/lib/workspace/ClaudeTuiTile.svelte`
  - visible resident-lane text
- `scripts/set-sdk-model.sh`
- `scripts/tui-a2a-inbox.sh`
- `scripts/tui-a2a-poller.sh`
- `scripts/test_tui_a2a_inbox.sh`

Status: mostly addressed in the first source scrub.

Completed:

- removed `bootstrap_nix_agent`, `build_nix_tui_slot`, and
  `ensure_nix_tui_profile_on_disk`;
- removed the startup task that auto-created private resident lanes;
- removed the main-page resident-lane special-case renderer;
- made the Claude TUI tile label generic;
- changed TUI persona generation to read from the spawning agent's own persona
  directory;
- changed default memory aliases to an empty map.

Still needs scan/validation:

- remaining generic tests may still use old household fixture names;
- TUI lane product docs need to be recreated in generic language.

Public direction:

- Replace resident-specific bootstrap with configurable example lanes.
- Keep externally-driven TUI lane mechanics if useful, but ship generic names and
  docs.
- Preserve private resident-lane behavior only in the private repository.

### B2: Remaining docs need allowlist triage

The private docs were intentionally removed from the public seed. Technical docs
should be reintroduced only after line-by-line public review.

Candidate public docs to recreate:

- architecture overview;
- local setup;
- agent lane model;
- memory model;
- tool surface model;
- A2A/Switchboard/Latch integration notes;
- safety and privacy boundaries.

### B3: Branding and package names still need full public review

Status: partially addressed.

Completed:

- Tauri product name, binary name, identifier, window title, and tray tooltip
  now say Holt;
- Node package name and description now say Holt;
- Agent SDK bridge now exposes public Holt MCP naming.
- inherited private branding assets were quarantined outside the repo.

Remaining examples:

- deeper comments/internal names;
- possible generated sidecar drift if the bridge is rebuilt differently.

Public direction:

- Rename presentation layer to Holt.
- Decide whether internal Rust module names remain transitional or are renamed.

### B4: `hillock-core` is still a path dependency

Status: resolved for the public seed.

Completed:

- copied reusable `hillock-core/src` into `crates/hillock-core/`;
- converted the crate manifest from workspace-inherited dependencies to
  explicit standalone dependencies;
- pointed the Tauri backend at `../../crates/hillock-core`;
- did not copy memory databases, generated caches, the old Hillock workspace,
  or private integration tests;
- scrubbed house-specific comments and test fixture IDs inside the vendored
  crate.

Former private path dependency:

```toml
hillock-core = { path = "../../../hillock/hillock-core" }
```

Current public-cut dependency:

```toml
hillock-core = { path = "../../crates/hillock-core" }
```

### B5: Provider/key pattern scan needs second pass

Status: resolved for the public seed.

Targeted review found no literal shipped secrets. The review did find
predecessor config/keychain migration behavior, which has been removed. Holt now
uses only its own `holt` config directory and `holt` keychain service.

### B6: Frontend dependency audit needs triage

`npm ci` completed in the isolated Holt app copy, but `npm audit` reported
dependency vulnerabilities inherited from the current frontend dependency graph.
No automatic audit fix was applied during this scrub because that can change
dependency versions and should be handled as an intentional modernization pass.

## Validation Snapshot

Completed before the first scrub checkpoint commit:

- Rust package formatting: `cargo fmt --manifest-path app/src-tauri/Cargo.toml --package holt --check`
- Rust tests: `cargo test --offline` from `app/src-tauri/`
  - result: 859 passed, 0 failed, 2 ignored
  - warnings retained: one unused helper and one unread config field
- Frontend diagnostics: `npm run check`
  - result: 0 errors, 0 warnings
- Whitespace gate: `git diff --cached --check`
- Scrub scan excluding ignored build/dependency/cache outputs:
  - private source provenance terms: no hits
  - resident identity names: no hits outside `NOTICE.md` legal credit exclusion
  - machine-local private paths and known password hints: no hits
- Vendored Hillock validation:
  - `cargo test --manifest-path crates/hillock-core/Cargo.toml --offline`
  - result: 87 passed, 0 failed

Not claimed:

- a full Tauri dev smoke test;
- a portable public build outside the maintainer machine;
- completion of the frontend dependency audit.

## Discuss-Later / Possible Back-Ports

These are not automatic back-ports to the private predecessor repository.

- Configurable resident-lane bootstrap may be cleaner than a private baked-in
  lane.
- Public docs may force clearer internal architecture boundaries.
- Scrub audit patterns may become a reusable release-readiness tool.

## Verification Commands

Current planned gates before first commit:

```sh
git status --short --ignored
rg -l -i 'mpfeifer|/home/|password|secret|token|api[_-]?key|private[_-]?key'
cargo test --offline
npm run check
```

The exact command list will be updated after the resident-lane scrub.
