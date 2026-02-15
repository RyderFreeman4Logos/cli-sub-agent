---
name = "mktsk"
description = "Convert TODO plans into Task tool entries for persistent serial execution across auto-compaction"
allowed-tools = "TaskCreate, TaskUpdate, TaskList, TaskGet, Read, Grep, Glob, Bash, Write, Edit"
tier = "tier-2-standard"
version = "0.1.0"
---

# mktsk: Make Task — Plan-to-Execution Bridge

Convert TODO plans into Task tool entries that persist across auto-compaction.
Strict serial execution: implement → review → commit → next.
Every task has executor tag, DONE WHEN condition, and commit step.

## Step 1: Parse TODO Plan

Read the TODO plan file (from mktd output or user-provided plan).
Extract each [ ] item with its executor tag and description.

## Step 2: Create Task Entries

## FOR task IN ${TODO_ITEMS}

## Step 2a: Create TaskCreate Entry

Create a TaskCreate entry for this TODO item.
Each task MUST include:
- Subject with executor tag: [Sub:developer], [Skill:commit], [CSA:tool]
- Description with clear scope
- DONE WHEN condition (mechanically verifiable)

## Step 2b: Append Commit Task

For implementation tasks, append a corresponding commit task:
[Skill:commit] → runs full commit workflow (fmt → lint → test → audit → review → commit)

## ENDFOR

## Step 4: Execute Tasks Serially

## FOR task IN ${TASK_LIST}

Execute tasks strictly in order. NEVER parallel development.
Only read-only/analysis tasks may run in parallel.

## IF ${TASK_IS_IMPLEMENTATION}

## Step 4a: Implement

Write code for the current task.

## Step 4b: Quality Gates

Tool: bash
OnFail: retry 2

```bash
just pre-commit
```

## Step 4c: Review

Tool: bash

```bash
csa review --diff
```

## Step 4d: Commit

## INCLUDE commit

Full commit workflow per logical unit.

## ENDIF

## IF ${CONTEXT_ABOVE_80_PERCENT}

## Step 4e: Compact Context

Preserve task list and decisions. Compact process details.

## ENDIF

Mark task complete via TaskUpdate.

## ENDFOR

## Step 5: Verify Completion

Check all tasks completed. Run final quality gates.

```bash
just pre-commit
git status
```
