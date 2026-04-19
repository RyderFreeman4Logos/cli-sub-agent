---
name: dev2merge
description: "Use when: full dev cycle branch->plan->implement->review->PR->merge"
allowed-tools: Bash, Read, Grep, Glob, Edit, Write
triggers:
  - "dev2merge"
  - "/dev2merge"
  - "dev-to-merge"
  - "/dev-to-merge"
  - "full dev cycle"
  - "implement and merge"
---

# Dev2Merge: Deterministic Development Pipeline

## Role Detection (READ THIS FIRST -- MANDATORY)

Role MUST be determined by explicit mode marker, not fragile natural-language substring matching.
Treat the run as executor ONLY when initial prompt contains:
`<skill-mode>executor</skill-mode>`.

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `../../PATTERN.md` relative to this `SKILL.md`, and follow it step by step.
3. **RECURSION GUARD**: Do NOT run `csa run --skill dev2merge` or `csa run --skill dev-to-merge` from inside this skill. Other `csa` commands required by the workflow (for example `csa run --skill mktd`, `csa review`, `csa debate`) are allowed. mktsk MUST be invoked directly (not via `csa run`) — see Step 8.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Execute the complete development lifecycle as a **deterministic weave workflow**.
Every stage has hard gates (`on_fail = "abort"`) — no step can be skipped by the LLM.

Pipeline: Branch Validation → FAST_PATH Detection → L1/L2 Quality Gates →
(FAST_PATH: commit → bump → review) or (Full: mktd → mktsk [direct, TaskCreate] → bump → cumulative review) →
Push Gate → PR Creation → pr-bot Hard Gate → Post-Merge Sync.

## Execution Protocol (ORCHESTRATOR ONLY)

<prompt-guard name="no-verify-forbidden">
ABSOLUTE PROHIBITION: You MUST NOT use `--no-verify` or `-n` with any `git commit` or `git push` command. All quality hooks (pre-commit, etc.) MUST be allowed to run. Bypassing hooks is a critical SOP violation. If hooks fail, fix the underlying code issues instead of bypassing.
</prompt-guard>

### Prerequisites

- Must be on a feature branch (not `main` or `dev`)

### Quick Start

```bash
csa plan run patterns/dev2merge/workflow.toml
```

Or invoke as a skill:

```bash
csa run --sa-mode true --skill dev2merge "Implement, review, and merge <scope description>"
```

### SA Mode Propagation (MANDATORY)

When operating under SA mode (e.g., dispatched by `/sa` or any autonomous workflow),
**ALL `csa` invocations MUST include `--sa-mode true`**. This includes `csa run`,
`csa review`, `csa debate`, and any other execution commands.

### Review/Debate Waiting Discipline (MANDATORY)

When a pipeline step requires review or debate, use the built-in command for the
matching intent:

- Review step -> `csa review`
- Debate step -> `csa debate`

Do NOT replace these with a hand-written `csa run` prompt unless the built-in
command is blocked by a concrete, documented error.

In slow Rust repositories, one healthy review/debate session taking 30-60
minutes is normal. Sparse early output or a `csa session wait` timeout is not
failure by itself.

If the original session is still healthy, keep waiting on the same session id.
Do NOT launch narrowed or duplicate review/debate sessions for the same scope
unless there is explicit crash/error evidence, persistent liveness failure, or
direct user instruction.

## While awaiting review/fix session

This is the while-waiting checklist. When you background a `csa session wait` via `run_in_background: true`, the next task-notification wakes you up automatically. Do not add sleep, hand-rolled polling, or a redundant wakeup on top.

**Safe parallel work**:
1. Draft the PR body or changelog entry for the current branch as local text only; do not run `gh pr create` yet.
2. For deferred MEDIUM findings from prior rounds, queue issue-template drafts locally and batch filing later when the review cluster is clear.
3. Read the next sprint task or issue to preload context for the next non-conflicting step.
4. Check existing issues for possible duplicate-of candidates for findings already queued.
5. Clean up stale TaskCreate or TaskUpdate entries.

**Do NOT**:
- Start new `csa run` or `csa review` sessions that could race on git branch or checkout state with the waiting one (single-checkout sequential rule, AGENTS.md 028).
- Edit source files while the main agent is acting as the Layer 0 orchestrator; that violates the SA-mode separation this wait is protecting.
- Run state-mutating git commands such as `git commit`, `git checkout <other-branch>`, or `git push`.
- Stack a ScheduleWakeup backup on top of the backgrounded wait; the task-notification is already the wake signal (AGENTS.md 042f).

If there is no useful parallel work available, return control and wait for the notification. Do not invent speculative work just to stay busy.

### Pipeline Steps

The workflow is compiled from the companion `../../PATTERN.md` file (relative to this `SKILL.md`) into `workflow.toml`.
All steps use `on_fail = "abort"`. Variables propagate via `CSA_VAR:KEY=value`.

| Step | Name | Gate | Tool |
|------|------|------|------|
| 1 | Validate Branch | Not main/dev | bash |
| 2 | FAST_PATH Detection | Diff-stat heuristic | bash |
| 3 | L1/L2 Quality Gates | `just fmt && just clippy` | bash |
| **IF FAST_PATH** | | | |
| 4 | Simplified Commit | `just test && git commit` | bash |
| 5 | Version Bump | `just bump-patch` if needed | bash |
| 6 | Pre-PR Review | `csa review --range` | bash |
| **ELSE (Full Pipeline)** | | | |
| 7 | Plan with mktd | `csa plan run patterns/mktd/workflow.toml` | bash |
| 8 | Execute with mktsk | Follow mktsk PATTERN.md directly (TaskCreate/TaskUpdate) | main agent |
| 9 | Version Bump | `just bump-patch` if needed | bash |
| 10 | Cumulative Review | `csa review --range main...HEAD` | bash |
| **ENDIF** | | | |
| 11 | Push Gate | `REVIEW_COMPLETED=true` required | bash |
| 12 | Create or Reuse PR | `gh pr create` or reuse existing, outputs `PR_NUMBER`/`PR_URL` | bash |
| 13 | pr-bot Hard Gate | **MANDATORY** — runs pr-bot (review + merge) | bash |
| 14 | Post-Merge Sync | Verifies PR MERGED, then `git checkout main && git merge --ff-only` | bash |

Steps 12-14 form the PR transaction. Step 12 creates the PR, Step 13 is a **hard gate**
that runs pr-bot (which performs cloud review and the actual merge). Step 14
verifies the PR reached MERGED state before syncing — this is defense in depth against
a skipped Step 13. Marker files provide idempotency in Step 13.

### FAST_PATH Heuristic

Changes are classified as FAST_PATH when:
- Only `.md`, `.txt`, `.lock`, `.toml` files changed (no code files)
- Total insertions < 100 lines

FAST_PATH skips mktd/mktsk/debate but **keeps** L1/L2 quality checks and pre-PR review.

### Physical Enforcement

A git pre-push hook at `scripts/hooks/pre-push` verifies that a `csa review` session
exists for the current HEAD before allowing push. Install:

```bash
ln -sf ../../scripts/hooks/pre-push .git/hooks/pre-push
```

## Example Usage

| Command | Effect |
|---------|--------|
| `/dev2merge` | Full deterministic pipeline for current branch |
| `/dev2merge scope="executor refactor"` | Pipeline with scope hint for mktd |
| `/dev-to-merge` | Backward-compatible alias |

## Integration

- **Composes**: `mktd` (planning + debate), `mktsk` (serial task execution), `commit` (per-task), `pr-bot` (review loop + merge)
- **Enforced by**: weave workflow compiler (`on_fail = "abort"`), git pre-push hook
- **Standalone**: Complete workflow — does not need other skills invoked separately

## Done Criteria

1. Feature branch validated (not main/dev).
2. FAST_PATH detection completed (heuristic applied).
3. `just fmt` and `just clippy` exit 0 (L1/L2 gates).
4. If full pipeline: mktd plan saved with `DONE WHEN` clauses, mktsk executed all tasks via main agent.
5. If FAST_PATH: simplified commit created with tests passing.
6. Version bumped if needed.
7. Pre-PR cumulative review passed (`REVIEW_COMPLETED=true`).
8. Push completed via `--force-with-lease` (pre-push hook verified review HEAD).
9. PR created or reused on GitHub targeting main, `PR_NUMBER` and `PR_URL` resolved.
10. pr-bot hard gate completed: either triggered `pr-bot` or detected an already-completed run for the same PR/HEAD.
11. PR state verified as MERGED (defense in depth against skipped Step 13).
12. Local main synced: `git fetch origin && git checkout main && git merge origin/main --ff-only`.
13. Feature branch cleaned up.
