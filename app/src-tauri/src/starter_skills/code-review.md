---
name: Code Review
description: Review code changes for quality and correctness
mode: inject
triggers:
  - keyword: "review"
  - keyword: "check this"
  - keyword: "look at"
priority: normal
max_tokens: 1000
---

# Code Review Checklist

When reviewing code changes:

## Correctness
- Does the code do what it claims?
- Are edge cases handled?
- Are error paths covered?

## Quality
- Are names clear and descriptive?
- Is the code DRY without being over-abstracted?
- Are functions focused (single responsibility)?

## Safety
- No hardcoded secrets or credentials?
- Input validated at system boundaries?
- No SQL injection, XSS, or command injection vectors?

## Testing
- Are there tests for the changes?
- Do tests verify behavior, not implementation details?
- Are failure cases tested?
