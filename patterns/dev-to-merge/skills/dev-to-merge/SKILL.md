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

Role MUST be determined by explicit mode marker, not fragile natural-language substring matching.
Treat the run as executor ONLY when initial prompt contains:
`<skill-mode>executor</skill-mode>`.

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
3. **Stage changes**: `git add -A`, then unstage incidental lockfiles unless scope indicates release/dependency updates.
4. **Security scan**: Grep staged files for hardcoded secrets.
5. **Security audit**: Run `security-audit` via bounded bash wrapper with timeout and required `SECURITY_AUDIT_VERDICT`.
6. **Pre-commit review**: Run `csa review --diff` (heterogeneous reviewer). Fix issues up to 3 rounds.
7. **Re-run quality gates**: `just pre-commit` after any fixes.
8. **Generate commit message**: Delegate to CSA (tier-1) for Conventional Commits.
9. **Commit**: `git commit -m "${COMMIT_MSG}"`.
10. **Version gate precheck**: auto-run `just check-version-bumped`; if needed, `just bump-patch` and create a dedicated release commit before pre-PR review/push.
11. **Pre-PR cumulative review**: `csa review --range main...HEAD` (covers full branch, NOT just last commit). MUST pass before push.
12. **Push**: `git push -u origin ${BRANCH}`.
13. **Create PR**: `gh pr create --base main`.
14. **Trigger codex bot**: post `@codex review` and capture trigger timestamp.
15. **Poll and evaluate**: wait for bot comments/reviews newer than trigger timestamp.
16. **Arbitrate false positives**: Use `csa debate` with independent model.
17. **Fix real issues**: Commit fixes, push, re-trigger bot (max 10 iterations).
18. **Merge**: `gh pr merge --squash --delete-branch`, update local main.

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
