---
name: dev-to-merge
description: Full development cycle from branch creation through mktd planning, commit, PR, codex-bot review, and merge
allowed-tools: Bash, Read, Grep, Glob, Edit, Write
triggers:
  - "dev-to-merge"
  - "/dev-to-merge"
  - "full dev cycle"
  - "implement and merge"
---

# Dev-to-Merge: End-to-End Development Workflow

## Role Detection (READ THIS FIRST -- MANDATORY)

Role MUST be determined by explicit mode marker, not fragile natural-language substring matching.
Treat the run as executor ONLY when initial prompt contains:
`<skill-mode>executor</skill-mode>`.

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `patterns/dev-to-merge/PATTERN.md` and follow it step by step.
3. **RECURSION GUARD**: Do NOT run `csa run --skill dev2merge` or `csa run --skill dev-to-merge` from inside this skill. Other `csa` commands required by the workflow (for example `csa run --skill mktd`, `csa review`, `csa debate`) are allowed.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Execute the complete development lifecycle on a feature branch: mandatory mktd planning (with internal debate), format, lint, test, stage, security scan, security audit, heterogeneous code review, commit with Conventional Commits, push, create PR, and then delegate the full cloud review/polling/fix/merge loop to `pr-codex-bot`. This is the "everything in one command" workflow that composes `mktd`, `commit`, `security-audit`, `ai-reviewed-commit`, and `pr-codex-bot` into a single end-to-end pipeline.

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
2. **Plan first (mktd)**: Run `csa run --skill mktd` and require a saved TODO for current branch (checkbox tasks + `DONE WHEN`). This guarantees mktd's built-in debate phase executed.
3. **Quality gates**: Run `just fmt`, `just clippy`, `just test` sequentially.
4. **Stage changes**: `git add -A`, then unstage incidental lockfiles unless scope indicates release/dependency updates.
5. **Security scan**: Grep staged files for hardcoded secrets.
6. **Security audit**: Run `security-audit` via bounded bash wrapper with timeout and required `SECURITY_AUDIT_VERDICT`.
7. **Pre-commit review**: Run `csa review --diff` (heterogeneous reviewer). Fix issues up to 3 rounds.
8. **Re-run quality gates**: `just pre-commit` after any fixes.
9. **Generate commit message**: Delegate to CSA (tier-1) for Conventional Commits.
10. **Commit**: `git commit -m "${COMMIT_MSG}"`.
11. **Version gate precheck**: auto-run `just check-version-bumped`; if needed, `just bump-patch` and create a dedicated release commit before pre-PR review/push.
12. **Pre-PR cumulative review**: `csa review --range main...HEAD` (covers full branch, NOT just last commit). MUST pass before push.
13. **Push**: `git push -u origin ${BRANCH}`.
14. **Create PR**: `gh pr create --base main`.
15. **Delegate PR review loop**: invoke `csa run --skill pr-codex-bot --no-stream-stdout ...`.
16. **Do not poll in caller**: all trigger/poll/timeout/fix/review/merge waiting is handled inside delegated CSA workflow.
17. **Post-merge sync**: ensure local `main` is updated after delegated workflow completes.

## Example Usage

| Command | Effect |
|---------|--------|
| `/dev-to-merge scope=executor` | Full cycle for executor module changes |
| `/dev-to-merge` | Full cycle for all current changes |
| `/dev2merge` | Preferred new command (same workflow behavior) |

## Integration

- **Composes**: `mktd`, `security-audit`, `ai-reviewed-commit` / `csa-review`, `commit`, `pr-codex-bot`
- **Uses**: `mktd` (mandatory planning + debate evidence), `debate` (false-positive arbitration and self-authored review)
- **Standalone**: Complete workflow -- does not need other skills to be invoked separately

## Done Criteria

1. Feature branch validated (not main/dev).
2. mktd plan completed and a branch TODO was saved (`DONE WHEN` present).
3. `just fmt`, `just clippy`, `just test` all exit 0.
4. Security scan found no hardcoded secrets.
5. Security audit returned PASS or PASS_DEFERRED.
6. Pre-commit review completed with zero unresolved P0/P1 issues.
7. Commit created with Conventional Commits format.
8. PR created on GitHub targeting main.
9. `pr-codex-bot` delegated workflow completed successfully.
10. Cloud review polling and fix loops stayed inside delegated CSA session (no caller-side polling loop).
11. PR merged via squash-merge.
12. Local main updated: `git checkout main && git pull origin main`.
13. Feature branch deleted (remote and local).
