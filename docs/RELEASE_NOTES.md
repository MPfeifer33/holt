# Holt Release Notes

## 2026-08-08 — Initial public research cut

Holt is now public as a fresh-history research harness for persistent,
local-first multi-agent orchestration.

This initial cut is intended for study, experimentation, and extension. It is
not a supported product release.

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
