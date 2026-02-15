# DEPRECATED: csa-analyze

This skill is deprecated and will be removed in a future version.

## Reason

The analysis delegation workflow is superseded by:
1. The `mktd` pattern (Phase 1 RECON) for structured exploration
2. Direct `csa run` invocation for ad-hoc analysis
3. The `csa-review` pattern for structured code review

The core principle (never pre-fetch data for CSA) is now documented in
AGENTS.md rule 004 and rule 020, making a dedicated skill unnecessary.

## Migration

- For codebase exploration: use `csa run "analyze <path>"`
- For structured planning: use the `mktd` pattern
- For code review: use the `csa-review` pattern
- For git change analysis: use `csa run "run git diff and analyze"`

## Timeline

- Deprecated: 2026-02-14
- Removal: after patterns are promoted from drafts/ to patterns/
