# Holt — Decision Record

Every consequential call in the public cut, with rationale. Amended as
decisions land; nothing here is retroactively rewritten.

Per Mark's directive 2026-08-08: judgment calls on the lanes for the public
research-harness cut, documented with rationale. Each entry: KEEP / GENERALIZE / EXCLUDE
+ why + what innovation (if any) goes on the discuss-later ledger.

## Lanes

1. **Nix bootstrap slot** (lib.rs ~230: protected OAuth/SDK lane, persona
   system prompt, hardcoded identity) — **GENERALIZE**: replace with a
   config-driven "primary agent" example lane, generic system prompt,
   `protected` demoted to a config flag. A research harness ships mechanisms,
   not residents. *Discuss-later:* the bootstrap-slot pattern itself
   (app-owned protected lane) is clean and worth documenting as a feature.
2. **nix-tui slot** (lib.rs ~454: Local:0 externally-driven placeholder
   lane) — **GENERALIZE + DOCUMENT**: the "externally-driven lane" concept
   (an agent whose turns come from outside the app) is genuinely novel
   research-harness material — rename to `external-lane` example, keep the
   Local:0 convention, write the doc that never existed. *Discuss-later:*
   that doc would help the main repo too.
3. **Codex lane machinery** (codex_mcp.rs, thread ops, full-trust ruling) —
   **KEEP mechanism / STRIP house policy**: the MCP bridge is core harness
   value; the Aug-4 "full trust, no approvals" posture is a HOUSE decision —
   public default reverts to approval-tiered.
4. **Archetype/persona registry** (persona_registry.rs: 6 bases, 37
   specializations, generic) — **KEEP AS-IS**: zero personal content; one of
   the harness's genuine selling points.
5. **Agent-SDK sidecar + session-fresh persona manifest** (agent_claude_config
   .rs) — **KEEP mechanism**, ship with example manifest; the session-fresh
   delivery design (yesterday's patch) is publishable research. House manifest
   values excluded.

## Non-lane surfaces

6. **OLD/Documents/** — **EXCLUDE WHOLESALE** (collab archive, pitch deck).
7. **PROJECT.md / CHANGELOG / DIAGNOSIS-*.md** — **EXCLUDE**; holt gets a
   fresh PROJECT.md written for the harness framing.
8. **docs/specs/** — per-file triage (Helix + me, checklist to follow):
   specs referencing house agents/sessions exclude; generic architecture
   specs keep with a scrub pass.
9. **Memory/Hillock integration** — **KEEP + VENDOR**: Hillock is part of
   Holt. Ship a scrubbed in-tree `hillock-core` snapshot with empty memory
   data rather than a private path dependency, public git dependency, or stub.
10. **Provider key handling** — mechanism is keychain-ref based (clean);
    Helix's scrub audit owns verification.

## Framing (README skeleton, for the repo)

- "Holt is a research harness for multi-agent orchestration — the public,
  identity-free cut of a private system." Position: mechanisms for lanes,
  personas-as-archetypes, memory interfaces, tool approval tiers, session
  persistence. NOT a product, NOT supported, NOT the private system's history.

## Discuss-later ledger (innovations → possible main-repo backports)

- (accumulating as the cut proceeds; nothing auto-applies to main.)


## Ratified rulings (2026-08-08)

- **Name:** holt (public/presentation); internal code nomenclature retained
  deliberately — no mass rename (closes audit B3's open question).
- **History:** fresh single-commit seed; predecessor history never ships.
- **License:** PolyForm Small Business 1.0.0 + NOTICE additional grant
  (unconditional personal noncommercial use) + commercial contact.
  Grow-then-pay ratified explicitly by the maintainer.
- **Memory engine (audit B4):** Hillock is PART of holt — vendored scrubbed
  `hillock-core` in-tree, not stubbed, not feature-flagged. "They're tied
  together." Memory data never ships; the engine arrives empty.
- **Publish switch:** maintainer-only, always.
- **Test fixtures:** house agent names replaced with `demo` (B1 validation
  complete, 859 tests green post-rename).
