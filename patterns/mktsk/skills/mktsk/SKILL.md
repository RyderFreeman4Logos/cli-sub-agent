---
name: mktsk
description: Convert TODO plans into deterministic, resumable serial execution checklists across auto-compaction
allowed-tools: Bash, Read, Grep, Glob, Write, Edit, TaskCreate, TaskUpdate, TaskGet, TaskList
triggers:
  - "mktsk"
  - "/mktsk"
  - "make tasks"
  - "execute plan"
  - "todo to tasks"
---

# mktsk: Make Task -- Plan-to-Execution Bridge

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the mktsk skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `patterns/mktsk/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Execute TODO plans (from `mktd` or user-provided) as deterministic, resumable serial checklists. Enforces strict serial execution: implement, verify, review, persist progress, then next task. Every checklist item carries an executor tag and a mechanically verifiable `DONE WHEN` condition.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `csa` binary MUST be in PATH: `which csa`
- A TODO plan must exist (from `/mktd` output or provided by user)
- Must be on a feature branch

### Quick Start

```bash
csa run --skill mktsk "Execute the TODO plan at <path or csa todo show -t <timestamp>>"
```

### Step-by-Step

1. **Parse TODO plan**: Read the TODO file. Extract each `[ ]` item with executor tag and `DONE WHEN`.
2. **Register tasks**: For each parsed TODO item, use TaskCreate to create a tracked task entry.
   Include the executor tag and `DONE WHEN` condition in the task description.
   TODO.md remains the read-only source of truth — mktsk reads from it, tracks progress via TaskCreate/TaskUpdate.
3. **Execute serially with checkpointing**: Process checklist items strictly in order. NEVER parallelize implementation tasks.
   - Before executing each item: use TaskUpdate to set its status to `in_progress`.
   - Treat each item as an atomic transaction: execute one item -> verify -> review -> persist checkpoint.
   - After each implementation item: run `just fmt`, `just clippy`, `just test`, then `csa review --diff`.
   - After completing each item: use TaskUpdate to set its status to `completed`.
   - Write latest completed item id to `.csa/state/mktsk/checkpoint.json` after each completed item.
   - On interruption, resume from unchecked TODO items and checkpoint state.
4. **Compact if needed**: If context > 80%, compact while preserving remaining items and review findings.
5. **Verify completion**: Run `just fmt`, `just clippy`, `just test`, and `git status --short`.

## Example Usage

| Command | Effect |
|---------|--------|
| `/mktsk` | Execute the most recent TODO plan for the current branch |
| `/mktsk path=./plans/feature.md` | Execute tasks from a specific plan file |
| `/mktsk timestamp=01JK...` | Execute tasks from a csa todo by timestamp |

## Integration

- **Depends on**: `mktd` (provides TODO plan), `commit` (per-task commit workflow)
- **Uses**: `csa-review` (per-task review), `security-audit` (via commit skill)
- **Part of**: Full planning pipeline: `mktd` (plan) -> `mktsk` (execute) -> `pr-codex-bot` (merge)

## Done Criteria

1. All TODO items parsed and registered via TaskCreate with executor tags.
2. All tasks executed in strict serial order with TaskUpdate status transitions.
3. Each task's DONE WHEN condition verified before marking complete.
4. Progress checkpoint is updated after each completed item.
5. `just fmt`, `just clippy`, and `just test` exit 0 after final task.
6. `git status` shows clean working tree.
7. All TaskCreate entries show status `completed` in TaskList.
