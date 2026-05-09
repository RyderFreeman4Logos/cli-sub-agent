---
name: recall
description: Recover main-agent context after `/clear`, `/compact`, or lost local thread state by using `csa recall` against recorded Claude main sessions.
---

# Recall

Use this skill when you need to recover recent main-agent context without dumping an entire thread into the current session.

## When to use

- After `/clear` or `/compact` when the current agent lost conversational context
- When you need to inspect what the main Claude thread was discussing
- When you want a targeted search over the most recent main-agent session

## Commands

```bash
csa recall list --limit 10
csa recall read latest
csa recall read 3 | tail -100
csa recall read <session-id> | rg "keyword"
csa recall search "keyword"
```

## Safe patterns

- Start with `csa recall list` to pick the right session.
- Prefer `csa recall read <selector> | tail -100` for large threads.
- Prefer `csa recall search "term"` before reading the full markdown.
- Treat numeric selectors as 1-based history indexes: `1` is the most recent session.

## Safety warnings

- Never paste a full recalled session back into the active prompt unless strictly necessary.
- Never run `csa recall read latest` unbounded on a large session; use `tail`, `rg`, or both.
- If `csa recall read` prints `OUTPUT_TOO_LARGE`, narrow the view instead of retrying the full dump.
