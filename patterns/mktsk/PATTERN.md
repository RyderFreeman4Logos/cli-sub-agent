---
name = "mktsk"
description = "Execute TODO plans as deterministic, resumable serial checklists across auto-compaction"
allowed-tools = "Read, Grep, Glob, Bash, Write, Edit, TaskCreate, TaskUpdate, TaskGet, TaskList"
tier = "tier-2-standard"
version = "0.3.0"
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

## Step 2: Register Tasks

TODO.md is a read-only planning artifact. Progress tracking uses TaskCreate/TaskUpdate.

Register each parsed TODO item via TaskCreate. Each task MUST include:
- stable item id
- source TODO line reference
- executor tag
- concrete action
- mechanically verifiable `DONE WHEN`

Do NOT modify TODO.md checkboxes — task progress is tracked via TaskUpdate status.

**MANDATORY publish-phase task decomposition** (when Step 6 applies):
The publish pipeline MUST be registered as **separate, individually tracked tasks**:
1. Push to remote
2. Create or reuse PR
3. **Run pr-bot** — SEPARATE task from PR creation. NEVER combine with step 2.
4. Post-merge local sync

pr-bot is the step that performs cloud review and the actual merge. Without a
dedicated task for it, agents consistently skip it after PR creation.

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

## Step 6: Publish Transaction

When mktsk is the top-level workflow (not called from dev2merge or another
parent), complete the full pipeline:

1. Ensure version bumped (`just bump-patch` if needed).
2. Run cumulative review (`csa review --range main...HEAD`).
3. Push to remote (`--force-with-lease`).
4. Create or reuse PR (`gh pr create`).
5. **Run pr-bot** (`csa plan run --sa-mode true patterns/pr-bot/workflow.toml`).

**CRITICAL — pr-bot is a SEPARATE step from PR creation:**
- NEVER skip pr-bot after creating a PR.
- NEVER merge the PR manually or via `gh pr merge`.
- NEVER consider the pipeline "done" after PR creation.
- pr-bot performs cloud review and the actual merge. It is the final gate.

**Skipped when**: `CSA_SKIP_PUBLISH=true` is set by the parent workflow.
dev2merge sets this before calling mktsk, since it handles publish in its
own Steps 10-13.

## Step 7: Post-Merge Local Sync

After PR merge, sync local main:

```bash
git fetch origin
git checkout main
git merge origin/main --ff-only
```

Skipped when `CSA_SKIP_PUBLISH=true`.
