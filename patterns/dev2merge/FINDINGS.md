# Dev2Merge Pattern: Compilation Findings

## Summary

The current `dev2merge` workflow compiles successfully and implements a
27-step end-to-end branch-to-merge pipeline with mandatory planning via
`mktd` (including mktd-internal debate), quality gates, local/cumulative
review, cloud codex review loop, and final merge.

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

- Added mandatory mktd planning gate before development gates.
- Migrated review handling from per-comment loop to consolidated analysis
  steps (better context and lower orchestration complexity).
- Hardened repository resolution with `gh repo view` primary path and
  remote URL fallback, including `.git` suffix normalization.
- Added top-level PR comments polling (in addition to inline comments and
  reviews) to reduce missed bot findings.
- Added explicit branch detection guards before push operations.

## Known Tradeoffs

- `REPO_LOCAL` resolution block is intentionally duplicated across multiple
  bash steps for robustness and local step self-sufficiency.
- Bot identity detection currently uses a heuristic login regex
  (`codex|bot|connector`) and may require updates if provider naming changes.

## Validation Snapshot

- `weave compile` succeeds for `patterns/dev2merge/PATTERN.md`.
- Local gates expected by this pattern (`fmt`, `clippy`, `test`, review)
  are runnable and integrated.
- Pattern and workflow definitions are synchronized for current behavior.
