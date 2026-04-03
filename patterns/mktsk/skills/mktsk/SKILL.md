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
