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

## Step 1: Variables

Tool: bash
OnFail: abort

Initialize and declare workflow variables.

- `${SESSION_ID}`: The ULID of the Layer 1 implementation session
- `${STATUS}`: Success/Failure status of the child session
- `${TODO_PATH}`: Path to the generated TODO.md file
- `${USER_APPROVES}`: Set to `"true"` if user approves the plan
- `${USER_MODIFIES}`: Set to `"true"` if user provides feedback for plan revision
- `${COMMIT_HASH}`: Resulting commit hash from implementation
- `${REVIEW_RESULT}`: Status of the implementation review
- `${REVIEW_IS_CLEAN}`: Set to `"true"` if review passed without issues
- `${FILES}`: Files to commit (passed to nested `commit` pattern)
- `${SCOPE}`: Commit scope (passed to nested `commit` pattern)

```bash
# Force weave to pick up these variables
: "${SESSION_ID}" "${STATUS}" "${TODO_PATH}" "${USER_APPROVES}" "${USER_MODIFIES}" "${COMMIT_HASH}" "${REVIEW_RESULT}" "${REVIEW_IS_CLEAN}" "${FILES}" "${SCOPE}"
echo "Variables initialized."
```

## Step 2: Validate Task Scope

Determine if sa is appropriate:
- Multi-step feature (planning + implementation) → use sa
- Cross-cutting concerns (>3 files) → use sa
- Wants heterogeneous review → use sa
- Single well-defined task → use csa run directly instead

## Step 3: Prepare Planning Prompt

Build planning prompt with user's requirements.
NEVER pre-read files — Layer 1 and Layer 2 read files natively.
Use mktemp for temp files (no race conditions).

```bash
PROMPT_FILE=$(mktemp /tmp/sa-planning-XXXXXX.txt)
echo "CSA_VAR:PROMPT_FILE=$PROMPT_FILE"
```

## Step 4: Dispatch Planning to Layer 1

Tool: bash
OnFail: abort

Layer 1 (claude-code) will:
1. Spawn up to 3 parallel Layer 2 workers for codebase exploration
2. Synthesize findings into TODO draft
3. Run adversarial debate via csa debate
4. Write `result.toml` to `$CSA_SESSION_DIR/result.toml` (with `todo_path = "$CSA_SESSION_DIR/artifacts/TODO.md"`)
5. Treat `csa session wait` timeout or sparse early output as a wait-state, not failure. In slow Rust repos, 30-60 minutes can be normal. Re-wait on the same session instead of launching duplicate planning/review/debate sessions.

```bash
SID=$(csa run --prompt-file "${PROMPT_FILE}")
scripts/csa/session-wait-until-done.sh "$SID"
```

## Step 5: Parse Planning Result

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

echo "CSA_VAR:SESSION_ID=$SESSION_ID"
echo "CSA_VAR:STATUS=$STATUS"
echo "CSA_VAR:TODO_PATH=$TODO_PATH"
```

## Step 6: Present TODO to User

Present the TODO path to user. Let them read and approve/modify.

## IF ${USER_APPROVES}

## Step 7: Dispatch Implementation to Layer 1

Tool: bash
OnFail: abort

Resume the Layer 1 session for implementation.
Layer 1 will: implement → delegate errors to Layer 2 → review → commit.

**Incremental review (MANDATORY)**: Before committing each TODO block,
Layer 1 MUST run `csa review --diff` on the uncommitted changes immediately.
After the review passes, commit the block. Do NOT accumulate all changes for
one cumulative review at the end. Small-scope reviews catch issues when
context is focused, avoiding large diffs where reviewers can only surface
2 findings per round.

**Patience rule (MANDATORY)**: If Layer 1 launches `csa review` or `csa debate`,
and `csa session wait` later times out or produces sparse early output, treat
that as a wait-state rather than an automatic failure. Continue waiting on the
same session id. Do NOT launch duplicate review/debate sessions for the same
scope unless there is explicit crash/error evidence, persistent liveness
failure, or user instruction.

COMMIT HOOK POLICY (MANDATORY): ABSOLUTE PROHIBITION on ALL hook bypass methods.
NEVER use `git commit --no-verify` or `git commit -n`. NEVER set `LEFTHOOK=0`
or `LEFTHOOK_SKIP` environment variables. NEVER use `env LEFTHOOK=0 git commit`
or `export LEFTHOOK=0` before git commands. NEVER modify `.git/hooks/*` files.
NEVER set `core.hooksPath` to bypass hooks. No exception unless the prompt
explicitly includes `ALLOW_GIT_COMMIT_NO_VERIFY=1`.
Bypassing hooks by ANY method is a critical SOP violation.
If hooks fail (including failures caused by unrelated workspace crates), STOP
and return a blocker / `needs_clarification` instead of bypassing hooks.
Fix the underlying issues to ensure codebase integrity. NEVER treat pre-existing
failures as justification for disabling hooks.

```bash
IMPL_FILE=$(mktemp /tmp/sa-impl-XXXXXX.txt)
echo "CSA_VAR:IMPL_FILE=$IMPL_FILE"
SID=$(csa run --session "${SESSION_ID}" --prompt-file "${IMPL_FILE}")
scripts/csa/session-wait-until-done.sh "$SID"
```

## ELSE

## IF ${USER_MODIFIES}

## Step 8: Resume with Feedback

Tool: bash

Resume Layer 1 with user's revision feedback.

```bash
RESUME_FILE=$(mktemp /tmp/sa-resume-XXXXXX.txt)
echo "CSA_VAR:RESUME_FILE=$RESUME_FILE"
SID=$(csa run --session "${SESSION_ID}" --prompt-file "${RESUME_FILE}")
scripts/csa/session-wait-until-done.sh "$SID"
```

## ELSE

## Step 9: Abandon Plan

User rejected. Stop and ask for new direction.

## ENDIF

## ENDIF

## Step 10: Parse Implementation Result

Tool: bash

Extract commit_hash, review_result, tasks_completed from CSA session `result.toml`.

```bash
RESULT_PATH="${LAST_LINE}"
RESULT_REAL=$(realpath "${RESULT_PATH}" 2>/dev/null) || { echo "result.toml path invalid: ${RESULT_PATH}" >&2; exit 1; }
[ -f "${RESULT_REAL}" ] || { echo "result.toml not found: ${RESULT_REAL}" >&2; exit 1; }
CSA_STATE_ROOT="${RESULT_REAL%/sessions/*/result.toml}"
[[ "${CSA_STATE_ROOT}" != "${RESULT_REAL}" ]] || { echo "Cannot derive state root: ${RESULT_REAL}" >&2; exit 1; }
COMMIT_HASH=$(grep -- 'commit_hash = ' "$RESULT_REAL" | cut -d'"' -f2)
REVIEW_RESULT=$(grep -- 'review_result = ' "$RESULT_REAL" | cut -d'"' -f2)

echo "CSA_VAR:COMMIT_HASH=$COMMIT_HASH"
echo "CSA_VAR:REVIEW_RESULT=$REVIEW_RESULT"
```

## Step 11: Report to User

Present implementation results: commit hash, review status,
number of tasks completed. If HAS_ISSUES, iterate.

## IF ${REVIEW_IS_CLEAN}

## Step 12: Auto PR Transaction

## INCLUDE commit

Evaluate whether to push and create PR (if milestone complete).
The nested `commit` pattern now owns the full PR transaction:
push → create/reuse PR → `scripts/hooks/post-pr-create.sh` → `pr-bot`.
Layer 1 executors MUST rely on that transaction instead of dispatching a
separate follow-up skill step.

## ENDIF
