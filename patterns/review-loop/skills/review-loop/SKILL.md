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

Bounded diagnostic review-fix front end. It may run at most 2 identical serial
`review → fix → review` rounds. If the branch is still not clean, the serial loop
MUST stop and change topology: freeze HEAD, fan out read-only discovery by
semantic scope × lens, verify/deduplicate, cluster root causes, batch repairs,
and finish with a fresh whole-range clean-room gate. Reaching the budget never
authorizes PASS, push, or merge.

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
   - **Multi-finding optimization**: When 2+ findings affect different files,
     uses `parallel-fix` pattern (parallel RECON / serial EDIT) for faster fix rounds.
3. If clean: exits successfully
4. If max rounds are reached: keeps the branch blocked, persists remaining
   findings, and escalates to convergence topology; never returns PASS merely
   because the serial budget is exhausted

## Variables

- `MAX_ROUNDS`: Maximum review-fix iterations (default: 2)
