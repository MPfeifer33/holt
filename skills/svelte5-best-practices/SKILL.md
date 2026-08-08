---
name: svelte5-best-practices
description: "Svelte 5 and SvelteKit best practices. Use when writing, reviewing, or refactoring Svelte 5 components. Covers runes, reactivity, styling, performance, and common anti-patterns."
---

# Svelte 5 Best Practices

Consolidated from the official Svelte AI docs and community skills. Use this when working on Holt's Svelte 5 / SvelteKit frontend.

## Runes

### $state
- Use only for variables that are reactive (trigger `$effect`, `$derived`, or template updates). Everything else should be a normal variable.
- Objects and arrays are deeply reactive but proxied. This has a performance cost.
- Use `$state.raw` for large objects that are only reassigned, never mutated. This is common for API responses, store data, and agent objects.
- Runes are top-level only. Do not nest inside functions or conditionals.

### $derived
- Prefer `$derived` over `$effect` for computed values. It takes an expression, not a function.
- Use `$derived.by(() => { ... })` for complex multi-step logic.
- `$derived` values can be reassigned (Svelte 5.25+). Use `const` if you want read-only.
- Always derive from `$props()` values to maintain reactivity when props change.

### $effect
- Treat as an escape hatch. Most things that feel like effects are better as `$derived`, event handlers, or `{@attach}`.
- Never update `$state` inside `$effect` if you can derive the value instead.
- Use `$effect` for: DOM side effects, external library integration, subscriptions, timers.
- Use `$inspect.trace()` as the first line of `$effect` or `$derived.by` to debug reactivity.

### $props
- Use `$props()` for component inputs. Destructure with defaults: `let { value = 0, label = '' } = $props();`
- Use `$bindable()` for two-way bindable props.
- Treat props as potentially changing. Derive dependent values from them.

## Events
- Use `onclick` not `on:click` (Svelte 5 syntax).
- Use `<svelte:window>` and `<svelte:document>` for window/document listeners instead of `onMount` or `$effect`.

## Snippets
- Use `{#snippet name()}` and `{@render name()}` instead of slots.
- Top-level snippets can be referenced in `<script>`.
- Snippets replace the old slot-based composition model entirely.

## Each Blocks
- Always use keyed `{#each}` blocks: `{#each items as item (item.id)}`.
- Keys must uniquely identify objects. Never use array indices as keys.
- Avoid destructuring in `{#each}` if you need to mutate items.

## Styling
- All `<style>` blocks are scoped to the component by default.
- Use `style:` directive for dynamic CSS values: `style:--columns={columns}`.
- Pass CSS custom properties as component props: `<Child --color="red" />`.
- Prefer CSS custom properties over `:global` for styling child components.
- Use `:global` only as a fallback for library components you don't control.

## Context
- Prefer `createContext` over `setContext`/`getContext` for type safety.
- Use context over shared module state to scope appropriately and prevent SSR leaks.

## Performance
- Use `$state.raw` for large, reassignment-only objects (agent data, API responses, message arrays).
- Debounce or RAF-gate DOM measurements (`scrollHeight`, `getBoundingClientRect`).
- Avoid redundant reactive computations. Compute once, derive from the result.
- Keep `$derived` chains short. Deep chains re-evaluate on every upstream change.
- For lists over 100 items, consider virtualization (only render visible items).
- Throttle pointer event handlers (drag, resize) with `requestAnimationFrame`.

## Anti-Patterns (Do Not Use)
- `on:click` (Svelte 4) -- use `onclick`
- `$:` reactive declarations (Svelte 4) -- use `$derived` or `$effect`
- `<slot />` -- use `{#snippet}` and `{@render}`
- `createEventDispatcher` -- use callback props
- Svelte stores for local state -- use `$state`
- `{@html}` without sanitization
- `$effect` for computed values (use `$derived`)
- Inline style strings -- use `style:` directive

## Holt-Specific Notes
- Agent objects in stores should use `$state.raw` (large, reassigned, not mutated).
- Chat message arrays grow large (up to 2500). Consider virtualization.
- Floating windows use pointer events for drag/resize. Always RAF-gate these handlers.
- The `+page.svelte` orchestrator is large. Minimize reactive dependencies that cross component boundaries.
- Use `$derived` for agent status checks, not repeated `.find()` calls on the agents array.

## Tooling
- `npx @sveltejs/mcp list-sections` -- list available Svelte 5 docs sections
- `npx @sveltejs/mcp get-documentation "$state,$derived,$effect"` -- fetch docs for specific topics
- `npx @sveltejs/mcp svelte-autofixer "<code>"` -- auto-fix Svelte code issues

## Sources
- [Svelte AI Docs - Skills](https://svelte.dev/docs/ai/skills)
- [svelte-skills-kit](https://github.com/spences10/svelte-skills-kit)
- Holt PERFORMANCE_AUDIT_2026-05-07.md
