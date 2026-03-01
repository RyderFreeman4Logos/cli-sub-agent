---
name: review-loop
description: Bounded iterative review-fix loop that reruns `csa review --diff` until clean or max rounds
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
csa run --skill review-loop "Review and fix current changes"
```

## Behavior

1. Reviews current diff with `csa review --diff`
2. If issues found: fixes them and re-reviews (up to MAX_ROUNDS)
3. If clean: exits successfully
4. If max rounds reached: reports remaining issues

## Variables

- `MAX_ROUNDS`: Maximum review-fix iterations (default: 2)
