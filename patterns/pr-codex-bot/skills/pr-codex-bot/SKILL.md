---
name: pr-codex-bot
description: Iterative PR review loop with cloud codex bot, local pre-PR audit, false-positive arbitration, and merge
allowed-tools: Bash, Read, Grep, Glob, Edit, Write
triggers:
  - "pr-codex-bot"
  - "/pr-codex-bot"
  - "codex bot review"
  - "PR bot"
  - "merge PR"
---

# PR Codex Bot: Two-Layer PR Review and Merge

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the pr-codex-bot skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `patterns/pr-codex-bot/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Orchestrate the full PR review-and-merge lifecycle with two-layer review: local pre-PR cumulative audit (covering main...HEAD) plus cloud codex bot review. Handles bot unavailability gracefully (local review is the foundation), performs false-positive arbitration via adversarial debate, and manages fix-push-retrigger loops up to 10 iterations. FORBIDDEN: self-dismissing bot comments or skipping debate for arbitration.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `csa` binary MUST be in PATH: `which csa`
- `gh` CLI MUST be authenticated: `gh auth status`
- All changes must be committed on a feature branch
- Feature branch must be ahead of main

### Quick Start

```bash
csa run --skill pr-codex-bot "Review and merge the current PR"
```

### Step-by-Step

1. **Commit check**: Ensure all changes are committed. Record `WORKFLOW_BRANCH`.
2. **Local pre-PR review** (SYNCHRONOUS -- MUST NOT background): Run `csa review --branch main` covering all commits since main. This is the foundation -- without it, bot unavailability cannot safely merge. Fix any issues found (max 3 rounds).
3. **Push and create PR**: `git push -u origin`, `gh pr create --base main`.
4. **Trigger cloud bot**: `gh pr comment --body "@codex review"`.
5. **Poll for bot response**: Bounded poll (max 10 minutes). If bot unavailable, proceed to merge (local review already covers).
6. **Evaluate bot comments**: Classify each as:
   - Category A (already fixed): react and acknowledge.
   - Category B (suspected false positive): arbitrate via `csa debate` with independent model. Post full audit trail to PR.
   - Category C (real issue): fix, commit, push.
7. **Re-trigger**: Push fixes and `@codex review` again. Loop (max 10 iterations).
8. **Clean resubmission** (if fixes accumulated): Create clean branch for final review.
9. **Merge**: `gh pr merge --squash --delete-branch`, then `git checkout main && git pull`.

## Example Usage

| Command | Effect |
|---------|--------|
| `/pr-codex-bot` | Full review loop on current branch's PR |
| `/pr-codex-bot pr=42` | Run review loop on existing PR #42 |

## Integration

- **Depends on**: `csa-review` (Step 2 local review), `debate` (Step 6 false-positive arbitration)
- **Used by**: `commit` (Step 13 auto PR), `dev-to-merge` (Steps 16-24)
- **ATOMIC with**: PR creation -- Steps 1-9 are an atomic unit; NEVER stop after PR creation

## Done Criteria

1. Local pre-PR review (`csa review --branch main`) completed synchronously (not backgrounded).
2. All local review issues fixed before PR creation.
3. PR created and cloud bot triggered.
4. Bot response received or timeout reached (bot unavailability handled gracefully).
5. Every bot comment classified (A/B/C) and actioned appropriately.
6. False positives arbitrated via `csa debate` with independent model; audit trail posted to PR.
7. Real issues fixed and re-reviewed.
8. PR merged via squash-merge with branch cleanup.
9. Local main updated: `git checkout main && git pull origin main`.
