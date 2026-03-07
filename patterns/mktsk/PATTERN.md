---
name = "mktsk"
description = "Execute TODO plans as deterministic, resumable serial checklists across auto-compaction"
allowed-tools = "Read, Grep, Glob, Bash, Write, Edit, TaskCreate, TaskUpdate, TaskGet, TaskList"
tier = "tier-2-standard"
version = "0.1.0"
---

# mktsk: Make Task — Plan-to-Execution Bridge

Execute TODO plans as deterministic serial checklists.
Strict order: implement -> verify -> review -> commit -> next.

## Step 1: Parse TODO Plan

Read the TODO plan file (from mktd output or user-provided path).
Extract all unchecked checklist items (`- [ ]`) with:
- executor tag (`[Sub:developer]`, `[Skill:commit]`, `[CSA:tool]`, or `[Main]`)
- task description
- `DONE WHEN:` condition

Fail fast if no executable checklist items are found.

## Step 2: Register Tasks via TaskCreate

TODO.md is a read-only planning artifact. Progress tracking uses TaskCreate/TaskUpdate.

For each parsed TODO item, use TaskCreate to register a tracked task entry.
Each task MUST include:
- stable item id
- source TODO line reference
- executor tag
- concrete action
- mechanically verifiable `DONE WHEN`

Do NOT modify TODO.md checkboxes — task progress is tracked via TaskUpdate status.

## Step 3: Execute Tasks Serially

Execute tasks strictly in order. Do not parallelize implementation work.

Before each task: use TaskUpdate to set status to `in_progress`.

Treat each task as an atomic transaction:
1. Execute exactly one item.
2. Run verification/review.
3. Use TaskUpdate to set status to `completed`.
4. Persist checkpoint before next item.

Dispatch policy by executor tag:
- `[CSA:tool]`: run `csa run` with the item objective and `DONE WHEN`.
- `[Sub:developer]`: execute directly as implementation work in current session.
- `[Skill:xxx]`: invoke the corresponding skill directly.
- `[Main]` or no tag: execute directly in current session.

Checkpoint policy (mandatory):
- Use `.csa/state/mktsk/checkpoint.json` as progress checkpoint.
- Write latest completed item id after each completed item.
- On interruption, resume from TaskList status and checkpoint state.

After each item:
1. Run quality gates (`just fmt`, `just clippy`, `just test`).
2. Run `csa review --diff`.
3. Apply fixes if review reports blocking issues.
4. Use TaskUpdate to mark the task as `completed`.

## Step 4: Compact Context (Conditional)

If context usage exceeds threshold, compact while preserving:
- remaining checklist items
- completed items
- open review findings
- pending `DONE WHEN` checks
- checkpoint path and latest completed item id

## Step 5: Verify Completion

Run final verification:

```bash
just fmt
just clippy
just test
git status --short
```

Ensure no unchecked executable checklist items remain in the TODO file.
