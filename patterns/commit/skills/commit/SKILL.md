---
name: commit
description: Strict commit discipline with security audit, test verification, code review, and quality gates
allowed-tools: Bash, Read, Grep, Glob, Edit
triggers:
  - "commit"
  - "/commit"
  - "audited commit"
---

# Commit: Audited Commit Workflow

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the commit skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `patterns/commit/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Enforce "Commit = Audited" discipline: every commit passes branch check, formatting, linting, tests, security audit, and heterogeneous code review before being created. Includes automatic PR creation when a logical milestone is reached, with pr-codex-bot integration for cloud review.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `csa` binary MUST be in PATH: `which csa`
- `just` MUST be in PATH: `which just`
- Must be on a feature branch (not `main` or `dev`)

### Quick Start

```bash
csa run --skill commit "Commit the current changes with scope: <scope>"
```

### Step-by-Step

1. **Branch check**: Verify on a feature branch (not main/dev). Abort if protected.
2. **Quality gates**: Run `just fmt`, `just clippy`, `just test` sequentially. Fix any failures.
3. **Stage changes**: `git add` relevant files. Verify no untracked files remain.
4. **Security scan**: Grep staged files for hardcoded secrets (API_KEY, SECRET, PASSWORD, PRIVATE_KEY).
5. **Security audit**: Invoke the `security-audit` pattern via CSA -- three-phase audit (test completeness, vulnerability scan, code quality).
6. **Pre-commit review**: Invoke the `ai-reviewed-commit` pattern via CSA -- authorship-aware review (debate for self-authored, `csa review --diff --allow-fallback` for others). Fix-and-retry up to 3 rounds.
7. **Generate commit message**: Delegate to CSA at `tier-1-quick` (tool and thinking budget come from config). If a review session already ran in this workflow, prefer resuming it with `--session <review-session-id>` (reuses cached context, near-zero new tokens). When resuming, keep the same tool (sessions are tool-locked).
8. **Commit**: `git commit -m "${COMMIT_MSG}"`.
9. **Auto PR** (if milestone): Push branch, create PR targeting main, invoke `/pr-codex-bot`.

## Example Usage

| Command | Effect |
|---------|--------|
| `/commit` | Commit current staged changes with full audit pipeline |
| `/commit scope=executor` | Commit with explicit scope for commit message |
| `/commit milestone=true` | Commit and automatically create PR + trigger codex-bot |

## Integration

- **Depends on**: `security-audit` (Step 5), `ai-reviewed-commit` (Step 6)
- **Triggers**: `pr-codex-bot` (Step 9, when milestone)
- **Used by**: `mktsk` (as commit step after each implementation task), `dev-to-merge`

## Done Criteria

1. Branch is a feature branch (not main/dev).
2. `just fmt`, `just clippy`, `just test` all exit 0.
3. No untracked files remain after staging.
4. Security scan found no hardcoded secrets.
5. Security audit returned PASS or PASS_DEFERRED.
6. Pre-commit review completed with zero unresolved P0/P1 issues.
7. Commit created with Conventional Commits format.
8. `git status` shows clean working tree.
9. If milestone: PR created and `/pr-codex-bot` invoked.
