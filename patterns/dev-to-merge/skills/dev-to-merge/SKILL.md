---
name: dev-to-merge
description: Full development cycle from branch creation through commit, PR, codex-bot review, and merge
allowed-tools: Bash, Read, Grep, Glob, Edit, Write
triggers:
  - "dev-to-merge"
  - "/dev-to-merge"
  - "full dev cycle"
  - "implement and merge"
---

# Dev-to-Merge: End-to-End Development Workflow

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the dev-to-merge skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `patterns/dev-to-merge/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Execute the complete development lifecycle on a feature branch: format, lint, test, stage, security scan, security audit, heterogeneous code review, commit with Conventional Commits, push, create PR, trigger cloud codex-bot review, handle false-positive arbitration via debate, fix-and-retrigger loops, and final squash-merge to main. This is the "everything in one command" workflow that composes `commit`, `security-audit`, `ai-reviewed-commit`, and `pr-codex-bot` into a single end-to-end pipeline.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `csa` binary MUST be in PATH: `which csa`
- `gh` CLI MUST be authenticated: `gh auth status`
- `just` MUST be in PATH: `which just`
- Must be on a feature branch (not `main` or `dev`)
- Code changes must exist (staged or unstaged)

### Quick Start

```bash
csa run --skill dev-to-merge "Implement, review, and merge <scope description>"
```

### Step-by-Step

1. **Validate branch**: Verify on feature branch, not main/dev. Abort if protected.
2. **Quality gates**: Run `just fmt`, `just clippy`, `just test` sequentially.
3. **Stage changes**: `git add -A`. Verify no untracked files.
4. **Security scan**: Grep staged files for hardcoded secrets.
5. **Security audit**: Run `security-audit` pattern via CSA (three phases).
6. **Pre-commit review**: Run `csa review --diff` (heterogeneous reviewer). Fix issues up to 3 rounds.
7. **Re-run quality gates**: `just pre-commit` after any fixes.
8. **Generate commit message**: Delegate to CSA (tier-1) for Conventional Commits.
9. **Commit**: `git commit -m "${COMMIT_MSG}"`.
10. **Pre-PR cumulative review**: `csa review --range main...HEAD` (covers full branch, NOT just last commit). MUST pass before push.
11. **Push**: `git push -u origin ${BRANCH}`.
12. **Create PR**: `gh pr create --base main`.
13. **Trigger codex bot**: `gh pr comment --body "@codex review"`.
14. **Poll and evaluate**: Handle bot comments (already-fixed, false-positive, real issues).
15. **Arbitrate false positives**: Use `csa debate` with independent model.
16. **Fix real issues**: Commit fixes, push, re-trigger bot (max 10 iterations).
17. **Merge**: `gh pr merge --squash --delete-branch`, update local main.

## Example Usage

| Command | Effect |
|---------|--------|
| `/dev-to-merge scope=executor` | Full cycle for executor module changes |
| `/dev-to-merge` | Full cycle for all current changes |

## Integration

- **Composes**: `security-audit`, `ai-reviewed-commit` / `csa-review`, `commit`, `pr-codex-bot`
- **Uses**: `debate` (for false-positive arbitration and self-authored review)
- **Standalone**: Complete workflow -- does not need other skills to be invoked separately

## Done Criteria

1. Feature branch validated (not main/dev).
2. `just fmt`, `just clippy`, `just test` all exit 0.
3. Security scan found no hardcoded secrets.
4. Security audit returned PASS or PASS_DEFERRED.
5. Pre-commit review completed with zero unresolved P0/P1 issues.
6. Commit created with Conventional Commits format.
7. PR created on GitHub targeting main.
8. Cloud codex bot triggered and response handled.
9. All bot comments classified and actioned (fixed, arbitrated, or acknowledged).
10. PR merged via squash-merge.
11. Local main updated: `git checkout main && git pull origin main`.
12. Feature branch deleted (remote and local).
