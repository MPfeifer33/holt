# PROJECT.md — Holt

> Updated every commit. It reflects the current state of the codebase.

## What Holt Is

A local-first **research harness** for persistent multi-agent orchestration —
the public, identity-free cut of a private predecessor system. Holt exists so
the architecture and mechanisms can be studied, run, and extended without the
private system's history, residents, or data.

Not a product. Not supported. Rough edges are part of the exhibit.

**Naming note:** the presentation layer says Holt; some internal code retains
the predecessor's nomenclature by deliberate decision (see
`docs/DECISIONS.md`). A holt is a den. The code is what lives in it.

## License

PolyForm Small Business 1.0.0 (`LICENSE.md`) + `NOTICE.md` (required notice,
an additional unconditional grant for personal noncommercial use, and
commercial licensing contact). Short version: credit required; personal and
small-business use free; sell it at scale, pay the licensor.

## Tech Stack

- **Backend:** Rust (Tauri 2), tokio async, SQLite (traces, memory)
- **Frontend:** SvelteKit + Svelte 5, desktop-first
- **Memory:** vendored `hillock-core` (in progress — see docs/PUBLIC_CUT_AUDIT.md B4)
- **Agent connectivity:** OpenAI-compatible / Anthropic / local endpoints,
  MCP servers, Agent SDK sidecar bridge

## Repo Layout

| Path | What |
|---|---|
| `app/src-tauri/` | Rust backend: runtime, providers, tools, persistence |
| `app/src/` | SvelteKit frontend |
| `docs/` | Public documentation + `PUBLIC_CUT_AUDIT.md` + `DECISIONS.md` |
| `skills/` | Skill registry content |
| `scripts/` | Dev scripts (public-safe subset) |

## Build / Run / Test

```bash
npm install                # frontend deps
npm run tauri dev          # dev app
npm run tauri build        # full build (frontend + sidecar + binary)
cargo test --manifest-path app/src-tauri/Cargo.toml   # backend tests
npm run check              # frontend type/lint check
```

## Current State

- Proper front page landed (README v2). Fresh-history public cut initialized from the private predecessor's main
  (provenance recorded outside this repo). Scrub audit:
  `docs/PUBLIC_CUT_AUDIT.md` — five blocker classes, B1/B3/B5 resolved,
  B2 (public docs) in progress, B4 (vendor memory engine) in progress.
- Backend tests: 859 passing. Frontend check: clean. Memory-system
  representation verified against hillock-core source (rotation.rs).
- Repo is **private until the maintainer explicitly flips it public.**

## Conventions

- Composition over inheritance.
- PROJECT.md updated every commit.
- No CI theater: local testing before push is the CI.
- Evidence before claims; decisions recorded in `docs/DECISIONS.md`.
