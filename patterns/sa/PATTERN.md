---
name = "sa"
description = "Three-layer recursive delegation: Layer 0 dispatches, Layer 1 plans/implements, Layer 2 explores/fixes"
allowed-tools = "Bash, Read, Grep, Glob, Task"
tier = "tier-3-complex"
version = "0.1.0"
---

# sa: Sub-Agent Orchestration (Three-Layer Architecture)

Layer 0 (main agent) dispatches. Layer 1 (claude-code) plans and implements.
Layer 2 (codex) explores and fixes errors. Each tier has its own context window.

Layer 0 NEVER reads source files — only reads session metadata from
the CSA session `result.toml` (path returned by CSA as last output line).
Heterogeneous review mandatory: author and reviewer must be different tools.

## Step 1: Validate Task Scope

Determine if sa is appropriate:
- Multi-step feature (planning + implementation) → use sa
- Cross-cutting concerns (>3 files) → use sa
- Wants heterogeneous review → use sa
- Single well-defined task → use csa run directly instead

## Step 2: Prepare Planning Prompt

Build planning prompt with user's requirements.
NEVER pre-read files — Layer 1 and Layer 2 read files natively.
Use mktemp for temp files (no race conditions).

```bash
PROMPT_FILE=$(mktemp /tmp/sa-planning-XXXXXX.txt)
```

## Step 3: Dispatch Planning to Layer 1

Tool: bash
OnFail: abort

Layer 1 (claude-code) will:
1. Spawn up to 3 parallel Layer 2 workers for codebase exploration
2. Synthesize findings into TODO draft
3. Run adversarial debate via csa debate
4. Write `result.toml` to `$CSA_SESSION_DIR/result.toml` (with `todo_path = "$CSA_SESSION_DIR/artifacts/TODO.md"`)

```bash
csa run < "${PROMPT_FILE}"
```

## Step 4: Parse Planning Result

Tool: bash

Extract session_id, status, and todo_path from CSA session `result.toml`.
Validate result and TODO paths stay inside CSA state directories.

```bash
# RESULT_PATH comes from CSA structured output (trusted: we invoked csa run)
RESULT_PATH="${LAST_LINE}"
RESULT_REAL=$(realpath "${RESULT_PATH}" 2>/dev/null) || { echo "result.toml path invalid: ${RESULT_PATH}" >&2; exit 1; }
[ -f "${RESULT_REAL}" ] || { echo "result.toml not found: ${RESULT_REAL}" >&2; exit 1; }
# Derive CSA state root: strip /sessions/<id>/result.toml suffix
CSA_STATE_ROOT="${RESULT_REAL%/sessions/*/result.toml}"
[[ "${CSA_STATE_ROOT}" != "${RESULT_REAL}" ]] || { echo "Cannot derive state root: ${RESULT_REAL}" >&2; exit 1; }
SESSION_ID=$(grep -- 'session_id = ' "$RESULT_REAL" | cut -d'"' -f2)
STATUS=$(grep -- 'status = ' "$RESULT_REAL" | head -1 | cut -d'"' -f2)
TODO_PATH=$(grep -- 'todo_path = ' "$RESULT_REAL" | cut -d'"' -f2)
TODO_REAL=$(realpath "${TODO_PATH}" 2>/dev/null) || { echo "TODO path invalid: ${TODO_PATH}" >&2; exit 1; }
[ -f "${TODO_REAL}" ] || { echo "TODO not found: ${TODO_REAL}" >&2; exit 1; }
[[ "${TODO_REAL}" == "${CSA_STATE_ROOT}"/todos/*/TODO.md ]] || { echo "TODO escapes state root: ${TODO_REAL}" >&2; exit 1; }
```

## Step 5: Present TODO to User

Present the TODO path to user. Let them read and approve/modify.

## IF ${USER_APPROVES}

## Step 6: Dispatch Implementation to Layer 1

Tool: bash
OnFail: abort

Resume the Layer 1 session for implementation.
Layer 1 will: implement → delegate errors to Layer 2 → review → commit.

**Incremental review (MANDATORY)**: After each TODO block is committed,
Layer 1 MUST run `csa review --diff` on that commit immediately — do NOT
accumulate all changes for one cumulative review at the end. Small-scope
reviews catch issues when context is focused, avoiding large diffs where
reviewers can only surface 2 findings per round.

```bash
IMPL_FILE=$(mktemp /tmp/sa-impl-XXXXXX.txt)
csa run --session "${SESSION_ID}" < "${IMPL_FILE}"
```

## ELSE

## IF ${USER_MODIFIES}

## Step 6a: Resume with Feedback

Tool: bash

Resume Layer 1 with user's revision feedback.

```bash
RESUME_FILE=$(mktemp /tmp/sa-resume-XXXXXX.txt)
csa run --session "${SESSION_ID}" < "${RESUME_FILE}"
```

## ELSE

## Step 6b: Abandon Plan

User rejected. Stop and ask for new direction.

## ENDIF

## ENDIF

## Step 7: Parse Implementation Result

Tool: bash

Extract commit_hash, review_result, tasks_completed from CSA session `result.toml`.

```bash
RESULT_PATH="${LAST_LINE}"
RESULT_REAL=$(realpath "${RESULT_PATH}" 2>/dev/null) || { echo "result.toml path invalid: ${RESULT_PATH}" >&2; exit 1; }
[ -f "${RESULT_REAL}" ] || { echo "result.toml not found: ${RESULT_REAL}" >&2; exit 1; }
CSA_STATE_ROOT="${RESULT_REAL%/sessions/*/result.toml}"
[[ "${CSA_STATE_ROOT}" != "${RESULT_REAL}" ]] || { echo "Cannot derive state root: ${RESULT_REAL}" >&2; exit 1; }
COMMIT=$(grep -- 'commit_hash = ' "$RESULT_REAL" | cut -d'"' -f2)
REVIEW=$(grep -- 'review_result = ' "$RESULT_REAL" | cut -d'"' -f2)
```

## Step 8: Report to User

Present implementation results: commit hash, review status,
number of tasks completed. If HAS_ISSUES, iterate.

## IF ${REVIEW_IS_CLEAN}

## Step 9: Auto PR

## INCLUDE commit

Evaluate whether to push and create PR (if milestone complete).

## Step 10: Invoke pr-codex-bot (MANDATORY)

> **Layer**: 0 (Orchestrator) -- dispatches /pr-codex-bot skill.
> Layer 1 executors MUST invoke /pr-codex-bot after PR creation.
> This is NOT optional — polling for bot review is part of the PR lifecycle.

Tool: skill
OnFail: abort

After PR creation, invoke the pr-codex-bot skill to trigger cloud review,
poll for response (with 10 min timeout), and handle the full review loop.
This ensures the bot review is never forgotten or skipped.

```
/pr-codex-bot
```

## ENDIF
