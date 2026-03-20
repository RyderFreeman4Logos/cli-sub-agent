---
name: review-loop
description: "Use when: iterative review-fix loop until csa review --diff is clean"
allowed-tools: Bash, Read, Grep, Edit, Write
triggers:
  - "review-loop"
  - "/review-loop"
  - "review and fix loop"
---

# Review Loop

Bounded iterative review-fix loop. Runs `csa review --diff` up to 2 rounds,
fixing issues between rounds until clean or max rounds exhausted.

## Usage

```bash
csa run --sa-mode true --skill review-loop "Review and fix current changes"
```

### SA Mode Propagation (MANDATORY)

When operating under SA mode (e.g., dispatched by `/sa` or any autonomous workflow),
**ALL `csa` invocations MUST include `--sa-mode true`**. This includes `csa run`,
`csa review`, `csa debate`, and any other execution commands. Omitting `--sa-mode`
at root depth causes a hard error; passing `false` when the caller is in SA mode
breaks prompt-guard propagation.

## Behavior

1. Reviews current diff with `csa review --diff`
2. If issues found: fixes them and re-reviews (up to MAX_ROUNDS)
3. If clean: exits successfully
4. If max rounds reached: reports remaining issues

## Variables

- `MAX_ROUNDS`: Maximum review-fix iterations (default: 2)
