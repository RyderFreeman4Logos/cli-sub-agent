---
name: mktsk
description: Convert TODO plans into Task tool entries for persistent serial execution across auto-compaction
allowed-tools: Bash, Read, Grep, Glob, Write, Edit
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

Convert TODO plans (from `mktd` or user-provided) into Task tool entries that persist across auto-compaction. Enforces strict serial execution: implement, review, commit, then next task. Every task carries an executor tag, a mechanically verifiable DONE WHEN condition, and a corresponding commit step. Context is compacted at >80% to prevent overflow during multi-task execution.

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

1. **Parse TODO plan**: Read the TODO file. Extract each `[ ]` item with its executor tag and description.
2. **Create Task entries**: For each TODO item, create a TaskCreate entry with:
   - Subject with executor tag: `[Sub:developer]`, `[Skill:commit]`, `[CSA:tool]`
   - Description with clear scope
   - DONE WHEN condition (mechanically verifiable)
3. **Append commit tasks**: For each implementation task, append a corresponding `[Skill:commit]` task that runs the full commit workflow.
4. **Execute serially**: Process tasks strictly in order. NEVER parallelize implementation tasks.
   - For each implementation task: write code, run `just pre-commit`, run `csa review --diff`, invoke `/commit`.
   - If context > 80%: compact, preserving task list and decisions.
   - Mark each task complete via TaskUpdate.
5. **Verify completion**: Check all tasks completed. Run `just pre-commit` and `git status` for final verification.

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

1. All TODO items parsed and converted to Task entries with executor tags.
2. Each implementation task has a corresponding commit task.
3. All tasks executed in strict serial order.
4. Each task's DONE WHEN condition verified before marking complete.
5. `just pre-commit` exits 0 after final task.
6. `git status` shows clean working tree.
7. All Task entries marked complete via TaskUpdate.
