---
name: mktsk
description: Convert TODO plans into deterministic serial execution checklists across auto-compaction
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

Execute TODO plans (from `mktd` or user-provided) as deterministic serial checklists. Enforces strict serial execution: implement, verify, review, commit, then next task. Every checklist item carries an executor tag and a mechanically verifiable `DONE WHEN` condition.

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
2. **Build checklist entries**: Normalize each item into a serial execution entry with:
   - Subject with executor tag: `[Sub:developer]`, `[Skill:commit]`, `[CSA:tool]`
   - Description with clear scope
   - DONE WHEN condition (mechanically verifiable)
3. **Execute serially**: Process checklist items strictly in order. NEVER parallelize implementation tasks.
   - After each implementation item: run `just fmt`, `just clippy`, `just test`, then `csa review --diff`.
   - Mark each completed item in the TODO checklist.
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

1. All TODO items parsed and converted to checklist entries with executor tags.
2. All checklist items executed in strict serial order.
3. Each task's DONE WHEN condition verified before marking complete.
4. Completed items are marked in TODO.
5. `just fmt`, `just clippy`, and `just test` exit 0 after final task.
6. `git status` shows clean working tree.
