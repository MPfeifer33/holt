---
name: Safe Shell Practices
description: Execute shell commands safely and defensively
mode: inject
triggers:
  - tools: [bash]
priority: high
max_tokens: 600
---

# Safe Shell Practices

When executing shell commands:

- **Quote all variables** — Use `"$var"` not `$var` to prevent word splitting
- **Check exit codes** — Use `set -e` or check `$?` after critical commands
- **Avoid rm -rf** without confirmation — Always verify paths before destructive operations
- **Use absolute paths** when possible to avoid working directory surprises
- **Pipe carefully** — `set -o pipefail` to catch failures in pipelines
- **Never execute untrusted input** — Don't pass user content directly to shell commands
