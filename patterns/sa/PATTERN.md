---
name = "sa"
description = "Three-tier recursive delegation: Tier 0 dispatches, Tier 1 plans/implements, Tier 2 explores/fixes"
allowed-tools = "Bash, Read, Grep, Glob, Task"
tier = "tier-3-complex"
version = "0.1.0"
---

# sa: Sub-Agent Orchestration (Three-Tier Architecture)

Tier 0 (main agent) dispatches. Tier 1 (claude-code) plans and implements.
Tier 2 (codex) explores and fixes errors. Each tier has its own context window.

Tier 0 NEVER reads source files — only reads session metadata from
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
NEVER pre-read files — Tier 1 and Tier 2 read files natively.
Use mktemp for temp files (no race conditions).

```bash
PROMPT_FILE=$(mktemp /tmp/sa-planning-XXXXXX.txt)
```

## Step 3: Dispatch Planning to Tier 1

Tool: bash
OnFail: abort

Tier 1 (claude-code) will:
1. Spawn up to 3 parallel Tier 2 workers for codebase exploration
2. Synthesize findings into TODO draft
3. Run adversarial debate via csa debate
4. Write `result.toml` (with `todo_path`) in CSA session state dir

```bash
csa run --tool claude-code < "${PROMPT_FILE}"
```

## Step 4: Parse Planning Result

Tool: bash

Extract session_id, status, and todo_path from CSA session `result.toml`.
Validate result and TODO paths stay inside CSA state directories.

```bash
RESULT_PATH="${LAST_LINE}"
RESULT_REAL=$(realpath -e "${RESULT_PATH}" 2>/dev/null) || { echo "result.toml not found: ${RESULT_PATH}" >&2; exit 1; }
# Derive CSA state root: strip /sessions/<id>/result.toml suffix
CSA_STATE_ROOT="${RESULT_REAL%/sessions/*/result.toml}"
[[ "${CSA_STATE_ROOT}" != "${RESULT_REAL}" ]] || { echo "Cannot derive state root: ${RESULT_REAL}" >&2; exit 1; }
SESSION_ID=$(grep 'session_id = ' "$RESULT_PATH" | cut -d'"' -f2)
STATUS=$(grep 'status = ' "$RESULT_PATH" | head -1 | cut -d'"' -f2)
TODO_PATH=$(grep 'todo_path = ' "$RESULT_PATH" | cut -d'"' -f2)
TODO_REAL=$(realpath -e "${TODO_PATH}" 2>/dev/null) || { echo "TODO path not found: ${TODO_PATH}" >&2; exit 1; }
[[ "${TODO_REAL}" == "${CSA_STATE_ROOT}"/todos/*/TODO.md ]] || { echo "TODO escapes state root: ${TODO_REAL}" >&2; exit 1; }
```

## Step 5: Present TODO to User

Present the TODO path to user. Let them read and approve/modify.

## IF ${USER_APPROVES}

## Step 6: Dispatch Implementation to Tier 1

Tool: bash
OnFail: abort

Resume the Tier 1 session for implementation.
Tier 1 will: implement → delegate errors to Tier 2 → review → commit.

```bash
IMPL_FILE=$(mktemp /tmp/sa-impl-XXXXXX.txt)
csa run --tool claude-code --session "${SESSION_ID}" < "${IMPL_FILE}"
```

## ELSE

## IF ${USER_MODIFIES}

## Step 6a: Resume with Feedback

Tool: bash

Resume Tier 1 with user's revision feedback.

```bash
RESUME_FILE=$(mktemp /tmp/sa-resume-XXXXXX.txt)
csa run --tool claude-code --session "${SESSION_ID}" < "${RESUME_FILE}"
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
RESULT_REAL=$(realpath -e "${RESULT_PATH}" 2>/dev/null) || { echo "result.toml not found: ${RESULT_PATH}" >&2; exit 1; }
CSA_STATE_ROOT="${RESULT_REAL%/sessions/*/result.toml}"
[[ "${CSA_STATE_ROOT}" != "${RESULT_REAL}" ]] || { echo "Cannot derive state root: ${RESULT_REAL}" >&2; exit 1; }
COMMIT=$(grep 'commit_hash = ' "$RESULT_PATH" | cut -d'"' -f2)
REVIEW=$(grep 'review_result = ' "$RESULT_PATH" | cut -d'"' -f2)
```

## Step 8: Report to User

Present implementation results: commit hash, review status,
number of tasks completed. If HAS_ISSUES, iterate.

## IF ${REVIEW_IS_CLEAN}

## Step 9: Auto PR

## INCLUDE commit

Evaluate whether to push and create PR (if milestone complete).

## ENDIF
