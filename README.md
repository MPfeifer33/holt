# Holt

Holt is a local-first research harness for persistent human/agent collaboration.

It is being cut from a private predecessor codebase into a separate,
public-facing repository with fresh git history. The goal is to preserve useful
architecture and mechanisms while removing private house state, resident agent
identities, credentials, sessions, personal lore, and machine-specific
assumptions.

This repository is **private until the maintainer explicitly flips it public**.

## What Holt Is

Holt is intended to be a reference architecture and working research harness for
exploring:

- persistent named agent lanes;
- local-first memory and context rehydration;
- human-in-the-loop approvals and attention flows;
- agent-to-agent coordination;
- tool surfaces designed for agent ergonomics;
- desktop-first orchestration with inspectable state.

It is not currently packaged as a turnkey product. Expect rough edges while the
public cut is in progress.

## Public-Cut Rules

The private source repository remains untouched. Work in this repository should
follow these rules:

1. Fresh history only. Do not import the private repository's git history.
2. No private data: credentials, session logs, conversation fixtures, resident
   personas, machine-local paths, or household-only docs.
3. No baked-in resident identities. Example lanes must be generic and
   configurable.
4. Public docs should explain mechanisms, not private house lore.
5. Improvements discovered during cleanup go into a discuss-later ledger before
   any back-port to the private repository.
6. License posture is PolyForm Small Business 1.0.0 plus the repository NOTICE;
   do not change it without an explicit maintainer decision.

## Current Cut Status

Source snapshot:

- created from a private predecessor snapshot;
- exact private provenance is tracked outside this repository;
- public-cut repository: Holt.

Immediate work is tracked in:

- [Public Cut Audit](docs/PUBLIC_CUT_AUDIT.md)
- local coordination ledgers outside the public source tree

## Development

This section will be rewritten once the public cut is scrubbed and build-tested.
For now, treat the source as under active surgery.

The current application shape is:

- Rust + Tauri backend under `app/src-tauri/`
- Svelte frontend under `app/src/`
- bundled resources under `app/src-tauri/resources/`

Do not publish, tag, or announce this repository until the audit checklist is
complete.
