# Dev-to-Merge Pattern: Compilation Findings

## Summary

`dev-to-merge` is maintained as a backward-compatible alias of `dev2merge`.
It compiles successfully and reflects the same 27-step branch-to-merge
workflow, including mandatory `mktd` planning/debate gate and the codex-bot
review loop.

## Current Workflow Shape

1. Validate branch safety (no direct work on protected branch).
2. Enforce planning gate through `mktd`, then verify TODO artifacts
   (checkbox tasks + `DONE WHEN`).
3. Run `just fmt`, `just clippy`, and `just test`.
4. Stage changes with lockfile-aware guardrails.
5. Run security scan + `security-audit` gate.
6. Run local review (`csa review --diff`) and fix loop when needed.
7. Generate commit message and commit.
8. Push branch, create PR, and trigger cloud codex review.
9. Poll review response (inline comments + PR comments + reviews).
10. If findings exist: evaluate, arbitrate disputed items via debate,
    fix, rerun local review, push, retrigger bot.
11. If clean: merge PR.

## Key Improvements Captured

- Kept behavior aligned with `dev2merge` while preserving legacy command
  compatibility.
- Added mandatory mktd planning gate before development gates.
- Migrated review handling from per-comment loop to consolidated analysis
  steps.
- Hardened repository resolution with `gh repo view` primary path and
  remote URL fallback, including `.git` suffix normalization.
- Added top-level PR comments polling to reduce missed bot findings.
- Added explicit branch detection guards before push operations.

## Known Tradeoffs

- `REPO_LOCAL` resolution block is duplicated across several bash steps for
  step-level self-containment.
- Bot identity detection still depends on login-name heuristics and may need
  tuning when external naming changes.

## Validation Snapshot

- `weave compile` succeeds for `patterns/dev-to-merge/PATTERN.md`.
- Local gates expected by this pattern are runnable and integrated.
- Alias remains functionally synchronized with `dev2merge`.
