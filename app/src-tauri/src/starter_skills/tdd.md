---
name: Test-Driven Development
description: Write tests before implementation code
mode: inject
triggers:
  - keyword: "implement"
  - keyword: "add feature"
  - keyword: "fix bug"
priority: normal
max_tokens: 800
---

# Test-Driven Development

When implementing any feature or fixing a bug:

1. **Write the failing test first** — Define what correct behavior looks like before writing code
2. **Run the test** — Confirm it fails for the right reason
3. **Write minimal implementation** — Just enough code to make the test pass
4. **Run tests again** — Confirm the test passes
5. **Refactor** — Clean up while keeping tests green

Always run the full test suite after changes to catch regressions.
