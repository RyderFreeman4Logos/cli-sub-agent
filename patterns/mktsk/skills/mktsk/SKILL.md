---
name: mktsk
description: "Use when: converting TODO plan into deterministic execution checklist"
allowed-tools: Bash, Read, Grep, Glob, Write, Edit, TaskCreate, TaskUpdate, TaskGet, TaskList
triggers:
  - "mktsk"
  - "/mktsk"
  - "make tasks"
  - "execute plan"
  - "todo to tasks"
---

# mktsk: Make Task -- Plan-to-Execution Bridge

## MANDATORY: Main Agent Execution

**mktsk MUST be executed by the main agent (Claude Code).**
Do NOT delegate to `csa plan run --pattern mktsk` or `csa run --skill mktsk`.

**Why**: mktsk requires Claude Code tools (TaskCreate, TaskUpdate, TaskGet, TaskList)
for progress tracking across auto-compaction. CSA subprocesses cannot use these tools,
making task persistence impossible.

## Execution Protocol

Read the pattern at `../../PATTERN.md` (relative to this SKILL.md) and follow it
step by step. You are executing directly — every step runs in your context.

For `csa` commands within pattern steps (e.g., `csa review --diff`), add
`--sa-mode true` when operating under SA mode.

## While awaiting review/fix session

This is the while-waiting checklist. When you background a `csa session wait` via `run_in_background: true`, the next task-notification wakes you up automatically. Do not add manual sleep, polling, a redundant `ScheduleWakeup`, or `/loop` on top.

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
- Stack a ScheduleWakeup or /loop backup on top of the backgrounded wait; the task-notification is already the wake signal (AGENTS.md 042f / 046).

If there is no useful parallel work available, return control and wait for the notification. Do not invent speculative work just to stay busy.

## Example Usage

| Command | Effect |
|---------|--------|
| `/mktsk` | Execute the most recent TODO plan for the current branch |
| `/mktsk path=./plans/feature.md` | Execute tasks from a specific plan file |
| `/mktsk timestamp=01JK...` | Execute tasks from a csa todo by timestamp |

## Integration

- **Depends on**: `mktd` (provides TODO plan), `commit` (per-task commit workflow)
- **Uses**: `csa-review` (per-task review), `security-audit` (via commit skill)
- **Boundary**: Standalone mktsk completes the full pipeline (push/PR/pr-bot/merge).
  When called from dev2merge (`CSA_SKIP_PUBLISH=true`), publish steps are skipped.

## Done Criteria

1. All TODO items executed and verified via `DONE WHEN` conditions.
2. All tasks marked complete via TaskUpdate.
3. Branch pushed to remote.
4. PR created (or reused).
5. **pr-bot completed** — this is a SEPARATE gate from PR creation. NEVER mark
   pipeline as done after PR creation without running pr-bot. pr-bot performs
   cloud review and the actual merge.
   (Steps 3-5 skipped when `CSA_SKIP_PUBLISH=true`.)
