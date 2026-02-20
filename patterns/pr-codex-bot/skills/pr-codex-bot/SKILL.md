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

## Dispatcher Model

pr-codex-bot follows a 3-tier dispatcher architecture. The main agent never
performs implementation work directly -- it orchestrates sub-agents that do the
actual review, fixing, and merging.

### Tier 0 -- Orchestrator (Main Agent)

The main agent (Claude Code / human user) acts as a **pure dispatcher**:

- Reads SKILL.md and PATTERN.md to understand the workflow
- Dispatches each step to the appropriate sub-agent or tool
- Evaluates sub-agent results and decides next action (fix, retry, merge, abort)
- **NEVER reads or writes code directly** -- all code-touching work is delegated
- **NEVER runs `csa review` / `csa debate` itself** -- spawns a Tier 1 executor

### Tier 1 -- Executor Sub-Agents (CSA / Task Tool)

Tier 1 agents perform the actual work dispatched by Tier 0:

| Step | Tier 1 Agent | Work Performed |
|------|-------------|----------------|
| Step 2 | `csa review --branch main` | Cumulative local review |
| Step 3 | `csa` (executor) | Fix local review issues |
| Step 7 | claude-code (Task tool) | Classify bot comments |
| Step 8 | `csa debate` | False-positive arbitration |
| Step 9 | `csa` (executor) | Fix real issues |

Tier 1 agents have full file system access and can read/write code, run tests,
and interact with git. They receive a scoped task from Tier 0 and return
results.

### Tier 2 -- Sub-Sub-Agents (Spawned by Tier 1)

Tier 1 agents may spawn their own sub-agents for specific sub-tasks:

- `csa review` internally spawns reviewer model(s) for independent analysis
- `csa debate` spawns two independent models for adversarial evaluation
- Task tool sub-agents may use Grep/Glob for targeted code search

Tier 2 agents are invisible to Tier 0 -- the orchestrator only sees Tier 1
results.

### Flow Diagram

```
Tier 0 (Orchestrator)
  |
  +-- dispatch --> Tier 1: csa review --branch main
  |                  |
  |                  +-- spawn --> Tier 2: reviewer model(s)
  |
  +-- evaluate result, decide next step
  |
  +-- dispatch --> Tier 1: csa (fix issues)
  |
  +-- dispatch --> Tier 1: bash (push, create PR, trigger bot)
  |
  +-- dispatch --> Tier 1: claude-code (classify comments)
  |
  +-- dispatch --> Tier 1: csa debate (arbitrate false positives)
  |                  |
  |                  +-- spawn --> Tier 2: independent models
  |
  +-- dispatch --> Tier 1: bash (merge)
```

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `csa` binary MUST be in PATH: `which csa`
- `gh` CLI MUST be authenticated: `gh auth status`
- All changes must be committed on a feature branch
- Feature branch must be ahead of main

### Configuration

The cloud bot trigger can be disabled per-project via `.csa/config.toml`:

```toml
[pr_review]
cloud_bot = false   # skip @codex cloud review, use local codex instead
```

**Check at runtime**: `csa config get pr_review.cloud_bot --default true`

When `cloud_bot = false`:
- Steps 4-9 (cloud bot trigger, poll, classify, arbitrate, fix) are **skipped entirely**
- An additional local review (`csa review --range main..HEAD`) replaces the cloud review
- The workflow proceeds directly to merge after local review passes
- This avoids the 10-minute polling timeout and GitHub API dependency

### Quick Start

```bash
csa run --skill pr-codex-bot "Review and merge the current PR"
```

### Step-by-Step

1. **Commit check**: Ensure all changes are committed. Record `WORKFLOW_BRANCH`.
2. **Local pre-PR review** (SYNCHRONOUS -- MUST NOT background): Run `csa review --branch main` covering all commits since main. This is the foundation -- without it, bot unavailability cannot safely merge. Fix any issues found (max 3 rounds).
3. **Push and create PR**: `git push -u origin`, `gh pr create --base main`.
3a. **Check cloud bot config**: Run `csa config get pr_review.cloud_bot --default true`.
    If `false` → skip Steps 4-9. Run `csa review --range main..HEAD` for supplementary
    local coverage, then jump to Step 11 (merge).
4. **Trigger cloud bot and poll** (SELF-CONTAINED -- trigger + poll are atomic):
   - Trigger `@codex review` (idempotent: skip if already commented on this HEAD).
   - Poll for bot response (max 10 minutes, 30s interval).
   - If bot times out: fallback to `csa review --range main..HEAD`, then proceed to merge.
5. **Evaluate bot comments**: Classify each as:
   - Category A (already fixed): react and acknowledge.
   - Category B (suspected false positive): queue for staleness filter, then arbitrate.
   - Category C (real issue): queue for staleness filter, then fix.
6. **Staleness filter** (before arbitration/fix): For each comment classified as B or C, check if the referenced code has been modified since the comment was posted. Compare comment file paths and line ranges against `git diff main...HEAD` and `git log --since="${COMMENT_TIMESTAMP}"`. Comments referencing lines changed after the comment timestamp are reclassified as Category A (potentially stale, already addressed) and skipped. This prevents debates and fix cycles on already-resolved issues.
7. **Arbitrate non-stale false positives**: For surviving Category B comments, arbitrate via `csa debate` with independent model. Post full audit trail to PR.
8. **Fix non-stale real issues**: For surviving Category C comments, fix, commit, push.
9. **Re-trigger**: Push fixes and re-trigger (loops back to step 4). Max 10 iterations.
10. **Clean resubmission** (if fixes accumulated): Create clean branch for final review.
11. **Merge**: `gh pr merge --squash --delete-branch`, then `git checkout main && git pull`.

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
3. PR created.
4. Cloud bot config checked (`csa config get pr_review.cloud_bot --default true`).
5. **If cloud_bot enabled (default)**: cloud bot triggered, response received or timeout handled.
6. **If cloud_bot disabled**: supplementary local review (`csa review --range main..HEAD`) run instead.
7. Every bot comment classified (A/B/C) and actioned appropriately (cloud_bot enabled only).
8. Staleness filter applied (cloud_bot enabled only).
9. Non-stale false positives arbitrated via `csa debate` (cloud_bot enabled only).
10. Real issues fixed and re-reviewed (cloud_bot enabled only).
11. PR merged via squash-merge with branch cleanup.
12. Local main updated: `git checkout main && git pull origin main`.
