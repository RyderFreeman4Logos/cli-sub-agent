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

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the mktsk skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `../../PATTERN.md` relative to this `SKILL.md`, and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Execute TODO plans (from `mktd` or user-provided) as deterministic, resumable serial checklists. Enforces strict serial execution: implement, verify, review, persist progress, then next task. Every checklist item carries an executor tag and a mechanically verifiable `DONE WHEN` condition.

mktsk stops at **verified local execution**. It does **not** push branches, create or reuse PRs, invoke `pr-codex-bot`, or merge. Those publication/integration steps are owned by the caller workflow (for example `dev2merge`, `/sa`, or another outer wrapper that continues after mktsk finishes).

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- A TODO plan must exist (from `/mktd` output or provided by user)
- Must be on a feature branch

### Quick Start

```bash
csa run --sa-mode true --skill mktsk "Execute the TODO plan at <path or csa todo show -t <timestamp>>"
```

### SA Mode Propagation (MANDATORY)

When operating under SA mode (e.g., dispatched by `/sa` or any autonomous workflow),
**ALL `csa` invocations MUST include `--sa-mode true`**. This includes `csa run`,
`csa review`, `csa debate`, and any other execution commands. Omitting `--sa-mode`
at root depth causes a hard error; passing `false` when the caller is in SA mode
breaks prompt-guard propagation.

### Step-by-Step

1. **Parse TODO plan**: Read the TODO file. Extract each `[ ]` item with executor tag and `DONE WHEN`.
   Check for associated references via `csa todo ref list` — if references exist
   (e.g., RECON findings, threat model), consult them for detailed context before
   executing tasks that need deeper understanding of the design rationale.
2. **Verify task registration**: When called from an orchestrator workflow (e.g., dev2merge),
   tasks are pre-registered by the orchestrator via TaskCreate before mktsk is invoked.
   Verify that tasks exist via TaskList. When called standalone (not as CSA subprocess),
   register tasks yourself via TaskCreate with executor tag and `DONE WHEN` condition.
   TODO.md remains the read-only source of truth — mktsk reads from it, tracks progress via TaskUpdate.
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
- **References**: Use `csa todo ref list` to discover plan references (RECON findings,
  debate evidence, threat model) and `csa todo ref show <name>` for selective loading
- **Boundary**: mktsk owns task execution, local verification, and progress persistence only. Push/PR/merge are caller-owned.
- **Part of**: Planning/execution segment of a larger pipeline: `mktd` (plan) -> `mktsk` (execute + verify locally). Any later publication flow, including `pr-codex-bot`, happens only if the caller invokes it.

## Done Criteria

1. All TODO items parsed and tasks verified (pre-registered by orchestrator or self-registered if standalone).
2. All tasks executed in strict serial order with TaskUpdate status transitions.
3. Each task's DONE WHEN condition verified before marking complete.
4. Progress checkpoint is updated after each completed item.
5. `just fmt`, `just clippy`, and `just test` exit 0 after final task.
6. `git status` shows clean working tree.
7. All TaskCreate entries show status `completed` in TaskList.
