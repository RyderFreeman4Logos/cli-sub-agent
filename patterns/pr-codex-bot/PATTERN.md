---
name = "pr-codex-bot"
description = "Iterative PR review loop with cloud codex bot: local review, push, bot trigger, false-positive arbitration, fix, merge"
allowed-tools = "Bash, Task, Read, Edit, Write, Grep, Glob"
tier = "tier-3-complex"
version = "0.1.0"
---

# PR Codex Bot Review Loop

Orchestrates iterative fix-and-review loop with cloud review bot on GitHub PRs.
Two-layer review: local pre-PR cumulative audit + cloud bot review.
Staleness guard: before arbitration, each bot comment is checked against the
latest HEAD to detect whether the referenced code has been modified since the
comment was posted. Stale comments (referencing already-modified code) are
reclassified as Category A and skipped, preventing wasted debate cycles on
already-fixed issues.

FORBIDDEN: self-dismissing bot comments, skipping debate for arbitration,
running Step 2 in background, creating PR without Step 2 completion,
debating stale comments without staleness check, trusting `reviewed=true`
without SHA verification, auto-merging or auto-aborting at round limit.

## Dispatcher Model Note

This pattern follows a 3-layer dispatcher architecture:
- **Layer 0 (Orchestrator)**: The main agent dispatches steps -- never touches code directly.
- **Layer 1 (Executors)**: CSA sub-agents and Task tool agents perform actual work.
- **Layer 2 (Sub-sub-agents)**: Spawned by Layer 1 for specific sub-tasks (invisible to Layer 0).

Each step below is annotated with its execution layer.

## Step 1: Commit Changes

> **Layer**: 0 (Orchestrator) -- lightweight shell command, no code reading.

Tool: bash

Ensure all changes committed. Set WORKFLOW_BRANCH once (persists through
clean branch switches in Step 11).

```bash
WORKFLOW_BRANCH="$(git branch --show-current)"
echo "CSA_VAR:WORKFLOW_BRANCH=${WORKFLOW_BRANCH}"
```

## Step 2: Local Pre-PR Review (SYNCHRONOUS — MUST NOT background)

> **Layer**: 1 (CSA executor) -- Layer 0 dispatches `csa review`, which spawns
> Layer 2 reviewer model(s) internally. Orchestrator waits for result.

Tool: bash
OnFail: abort

Run cumulative local review covering all commits since main.
This is the FOUNDATION — without it, bot unavailability cannot safely merge.

> Fast-path (SHA-verified): compare `git rev-parse HEAD` with the HEAD SHA
> stored in the latest `csa review` session metadata. If they match, skip
> Step 2 review. If they do not match (or metadata is missing), run full
> `csa review --branch main`. Any HEAD drift (including amend) auto-invalidates
> the fast-path.

```bash
set -euo pipefail
CURRENT_HEAD="$(git rev-parse HEAD)"
REVIEW_HEAD="$(csa session list --recent-review 2>/dev/null | parse_head_sha || true)"
if [ -n "${REVIEW_HEAD}" ] && [ "${CURRENT_HEAD}" = "${REVIEW_HEAD}" ]; then
  echo "Fast-path: local review already covers current HEAD."
else
  csa review --branch main
fi
REVIEW_COMPLETED=true
echo "CSA_VAR:REVIEW_COMPLETED=${REVIEW_COMPLETED}"
```

## IF ${LOCAL_REVIEW_HAS_ISSUES}

## Step 3: Fix Local Review Issues

> **Layer**: 1 (CSA executor) -- Layer 0 dispatches fix task to CSA. CSA reads
> code, applies fixes, and returns results. Orchestrator reviews outcome.

Tool: csa
Tier: tier-2-standard
OnFail: retry 3

Fix issues found by local review. Loop until clean (max 3 rounds).

## ENDIF

## Step 4: Push and Ensure PR

> **Layer**: 0 (Orchestrator) -- shell commands only, no code reading/writing.

Tool: bash
OnFail: abort

**PRECONDITION (MANDATORY)**: Step 2 local review MUST have completed successfully.
- If `REVIEW_COMPLETED` is not `true`, STOP and report:
  `ERROR: Local review (Step 2) was not completed. Run csa review --branch main before creating PR.`
- If Step 2 found unresolved issues that were not fixed in Step 3, STOP and report:
  `ERROR: Local review found unresolved issues. Fix them before PR creation.`
- FORBIDDEN: Creating a PR without completing Step 2. This violates the two-layer review guarantee.

```bash
# --- Precondition gate: review must be complete ---
if [ "${REVIEW_COMPLETED:-}" != "true" ]; then
  echo "ERROR: Local review (Step 2) was not completed."
  echo "Run csa review --branch main before creating PR."
  echo "FORBIDDEN: Creating PR without Step 2 completion."
  exit 1
fi

set -euo pipefail
git push -u origin "${WORKFLOW_BRANCH}"
ORIGIN_URL="$(git remote get-url origin)"
SOURCE_OWNER="$(
  printf '%s\n' "${ORIGIN_URL}" | sed -nE \
    -e 's#^https?://([^@/]+@)?github\\.com/([^/]+)/[^/]+(\\.git)?$#\\2#p' \
    -e 's#^(ssh://)?([^@]+@)?github\\.com[:/]([^/]+)/[^/]+(\\.git)?$#\\3#p' \
    | head -n 1
)"
if [ -z "${SOURCE_OWNER}" ]; then
  SOURCE_OWNER="$(gh repo view --json owner -q '.owner.login')"
fi
find_branch_pr() {
  local owner_matches owner_count
  owner_matches="$(
    gh pr list --base main --state open --head "${SOURCE_OWNER}:${WORKFLOW_BRANCH}" --json number \
      --jq '.[].number' 2>/dev/null || true
  )"
  owner_count="$(printf '%s\n' "${owner_matches}" | sed '/^$/d' | wc -l | tr -d ' ')"
  if [ "${owner_count}" = "1" ]; then
    printf '%s\n' "${owner_matches}" | sed '/^$/d' | head -n 1
    return 0
  fi
  if [ "${owner_count}" -gt 1 ]; then
    echo "ERROR: Multiple open PRs found for ${SOURCE_OWNER}:${WORKFLOW_BRANCH}. Resolve ambiguity manually." >&2
    return 1
  fi
  return 2
}

set +e
PR_NUM="$(find_branch_pr)"
FIND_RC=$?
set -e
if [ "${FIND_RC}" = "0" ] && [ -n "${PR_NUM}" ]; then
  echo "Using existing PR #${PR_NUM} for branch ${WORKFLOW_BRANCH}"
elif [ "${FIND_RC}" = "1" ]; then
  exit 1
else
  set +e
  CREATE_OUTPUT="$(gh pr create --base main --head "${SOURCE_OWNER}:${WORKFLOW_BRANCH}" --title "${PR_TITLE}" --body "${PR_BODY}" 2>&1)"
  CREATE_RC=$?
  set -e
  if [ "${CREATE_RC}" != "0" ]; then
    if ! printf '%s\n' "${CREATE_OUTPUT}" | grep -Eiq 'already exists|a pull request already exists'; then
      echo "ERROR: gh pr create failed: ${CREATE_OUTPUT}" >&2
      exit 1
    fi
    echo "Detected existing PR during create race; resolving PR number by owner+branch."
  fi
  FIND_RC=2
  PR_NUM=""
  for attempt in 1 2 3 4 5; do
    set +e
    PR_NUM="$(find_branch_pr)"
    FIND_RC=$?
    set -e
    if [ "${FIND_RC}" = "0" ] && [ -n "${PR_NUM}" ]; then
      break
    fi
    if [ "${FIND_RC}" = "1" ]; then
      break
    fi
    sleep 2
  done
  if [ "${FIND_RC}" != "0" ] || [ -z "${PR_NUM}" ]; then
    echo "ERROR: Failed to resolve a unique PR for branch ${WORKFLOW_BRANCH} targeting main." >&2
    exit 1
  fi
fi
if [ -z "${PR_NUM:-}" ] || ! printf '%s' "${PR_NUM}" | grep -Eq '^[0-9]+$'; then
  echo "ERROR: Failed to resolve PR number for branch ${WORKFLOW_BRANCH} targeting main." >&2
  exit 1
fi
REPO="$(gh repo view --json nameWithOwner -q '.nameWithOwner')"
echo "CSA_VAR:PR_NUM=${PR_NUM}"
echo "CSA_VAR:REPO=${REPO}"
```

## Step 4a: Check Cloud Bot Configuration

> **Layer**: 0 (Orchestrator) -- config check, lightweight.

Tool: bash
OnFail: abort

Check whether cloud bot review is enabled for this project.

```bash
set -euo pipefail
CLOUD_BOT=$(csa config get pr_review.cloud_bot --default true)
if [ "${CLOUD_BOT}" = "false" ]; then
  BOT_UNAVAILABLE=true
  FALLBACK_REVIEW_HAS_ISSUES=false
  CURRENT_HEAD="$(git rev-parse HEAD)"
  REVIEW_HEAD="$(csa session list --recent-review 2>/dev/null | parse_head_sha || true)"
  if [ -n "${REVIEW_HEAD}" ] && [ "${CURRENT_HEAD}" = "${REVIEW_HEAD}" ]; then
    echo "Cloud bot disabled, fast-path active: local review already covers HEAD ${CURRENT_HEAD}."
  else
    echo "Cloud bot disabled and fast-path invalid. Running full local review."
    csa review --branch main
  fi
fi
BOT_UNAVAILABLE="${BOT_UNAVAILABLE:-false}"
FALLBACK_REVIEW_HAS_ISSUES="${FALLBACK_REVIEW_HAS_ISSUES:-false}"
echo "CSA_VAR:BOT_UNAVAILABLE=${BOT_UNAVAILABLE}"
echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=${FALLBACK_REVIEW_HAS_ISSUES}"
```

If `CLOUD_BOT` is `false`:
- Skip Steps 5 through 10 (cloud bot trigger, delegated wait gate, classify, arbitrate, fix).
- Reuse the same SHA-verified fast-path before supplementary review:
  - If current `HEAD` matches latest reviewed session HEAD SHA → skip review.
  - Otherwise run full `csa review --branch main`.
- Route to Step 6a (Merge Without Bot) after supplementary local review gate passes.

## IF ${CLOUD_BOT} != "false"

## Step 5: Trigger Cloud Bot Review and Delegate Waiting

> **Layer**: 0 + 1 (Orchestrator + CSA executor).
> Layer 0 triggers `@codex review` and delegates the long wait to a single
> CSA-managed step. No explicit caller-side polling loop.

Tool: bash
OnFail: abort
Condition: !(${BOT_UNAVAILABLE})

Trigger a fresh `@codex review` for current HEAD, then delegate the waiting
window (max 10 minutes) to CSA.
If bot times out, set BOT_UNAVAILABLE and fall through — local review
(Step 2) already covers main...HEAD. If fallback review finds issues, set
`FALLBACK_REVIEW_HAS_ISSUES=true` so Step 6-fix is required before merge.

```bash
set -euo pipefail
TIMEOUT_BIN="$(command -v timeout || command -v gtimeout || true)"
run_with_hard_timeout() {
  local timeout_secs="$1"
  shift
  if [ -n "${TIMEOUT_BIN}" ]; then
    "${TIMEOUT_BIN}" -k 5s "${timeout_secs}s" "$@" 2>&1
    return $?
  fi

  local tmp_out timeout_flag child_pid watcher_pid rc use_pgroup
  tmp_out="$(mktemp)"
  timeout_flag="$(mktemp)"
  rm -f "${timeout_flag}"
  use_pgroup=false
  if command -v setsid >/dev/null 2>&1; then
    setsid "$@" >"${tmp_out}" 2>&1 &
    use_pgroup=true
    child_pid=$!
  else
    "$@" >"${tmp_out}" 2>&1 &
    child_pid=$!
  fi
  (
    sleep "${timeout_secs}"
    if kill -0 "${child_pid}" 2>/dev/null; then
      : >"${timeout_flag}"
      if [ "${use_pgroup}" = "true" ]; then
        kill -TERM "-${child_pid}" 2>/dev/null || true
      else
        kill -TERM "${child_pid}" 2>/dev/null || true
      fi
      sleep 2
      if kill -0 "${child_pid}" 2>/dev/null; then
        if [ "${use_pgroup}" = "true" ]; then
          kill -KILL "-${child_pid}" 2>/dev/null || true
        else
          kill -KILL "${child_pid}" 2>/dev/null || true
        fi
      fi
    fi
  ) &
  watcher_pid=$!
  wait "${child_pid}"
  rc=$?
  kill "${watcher_pid}" 2>/dev/null || true
  cat "${tmp_out}"
  rm -f "${tmp_out}"
  if [ -f "${timeout_flag}" ]; then
    rm -f "${timeout_flag}"
    return 124
  fi
  rm -f "${timeout_flag}"
  return "${rc}"
}

# --- Trigger fresh @codex review for current HEAD ---
CURRENT_SHA="$(git rev-parse HEAD)"
TRIGGER_TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
WAIT_BASE_TS="${TRIGGER_TS}"
TRIGGER_BODY="@codex review

<!-- csa-trigger:${CURRENT_SHA}:${TRIGGER_TS} -->"
gh pr comment "${PR_NUM}" --repo "${REPO}" --body "${TRIGGER_BODY}"

# --- Delegate wait to CSA-managed step (max 10 min) ---
BOT_UNAVAILABLE=true
FALLBACK_REVIEW_HAS_ISSUES=false
set +e
WAIT_RESULT="$(run_with_hard_timeout 650 csa run --tool codex --idle-timeout 650 "Bounded wait task only. Do NOT invoke pr-codex-bot skill or any full PR workflow. Operate on PR #${PR_NUM} in repo ${REPO}. Wait for @codex review response posted after ${WAIT_BASE_TS} for HEAD ${CURRENT_SHA}. Max wait 10 minutes. Do not edit code. Return exactly one marker line: BOT_REPLY=received or BOT_REPLY=timeout.")"
WAIT_RC=$?
set -e
if [ "${WAIT_RC}" -eq 124 ]; then
  BOT_UNAVAILABLE=true
elif [ "${WAIT_RC}" -ne 0 ]; then
  echo "WARN: Delegated bot wait failed (rc=${WAIT_RC}); treating cloud bot as unavailable." >&2
  BOT_UNAVAILABLE=true
else
  WAIT_MARKER="$(
    printf '%s\n' "${WAIT_RESULT}" \
      | grep -E '^[[:space:]]*BOT_REPLY=(received|timeout)[[:space:]]*$' \
      | tail -n 1 \
      | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//' \
      || true
  )"
  if [ "${WAIT_MARKER}" = "BOT_REPLY=received" ]; then
    BOT_UNAVAILABLE=false
  elif [ "${WAIT_MARKER}" = "BOT_REPLY=timeout" ]; then
    BOT_UNAVAILABLE=true
  else
    echo "WARN: Delegated bot wait returned no marker; treating cloud bot as unavailable." >&2
    BOT_UNAVAILABLE=true
  fi
fi

if [ "${BOT_UNAVAILABLE}" = "true" ]; then
  echo "Bot timed out after delegated wait window. Falling back to local review."
  if ! csa review --range main...HEAD --timeout 1200 2>/dev/null; then
    FALLBACK_REVIEW_HAS_ISSUES=true
  fi
fi
echo "CSA_VAR:BOT_UNAVAILABLE=${BOT_UNAVAILABLE}"
echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=${FALLBACK_REVIEW_HAS_ISSUES}"
```

## IF ${BOT_UNAVAILABLE}

## Step 6-fix: Fallback Review Fix Cycle (Bot Timeout Path)

> **Layer**: 0 + 1 (Orchestrator + CSA executor) -- wrapper step enforces
> timeout/return-code/marker contracts, while CSA performs the fix cycle.

Tool: bash
OnFail: abort

When `FALLBACK_REVIEW_HAS_ISSUES=true` (set in Step 5 when `csa review`
found issues during bot timeout), this dedicated fix cycle runs WITHIN the
timeout branch. Steps 7-10 are structurally inside the `BOT_UNAVAILABLE=false`
branch and are NOT reachable from here.

Delegate this cycle to CSA as a single operation and enforce hard bounds:
- command-level hard timeout: `timeout 1800s`
- no `|| true` silent downgrade
- success requires marker `FALLBACK_FIX=clean`
- on success, orchestrator explicitly sets `FALLBACK_REVIEW_HAS_ISSUES=false`

```bash
set -euo pipefail
TIMEOUT_BIN="$(command -v timeout || command -v gtimeout || true)"
run_with_hard_timeout() {
  local timeout_secs="$1"
  shift
  if [ -n "${TIMEOUT_BIN}" ]; then
    "${TIMEOUT_BIN}" -k 5s "${timeout_secs}s" "$@" 2>&1
    return $?
  fi

  local tmp_out timeout_flag child_pid watcher_pid rc use_pgroup
  tmp_out="$(mktemp)"
  timeout_flag="$(mktemp)"
  rm -f "${timeout_flag}"
  use_pgroup=false
  if command -v setsid >/dev/null 2>&1; then
    setsid "$@" >"${tmp_out}" 2>&1 &
    use_pgroup=true
    child_pid=$!
  else
    "$@" >"${tmp_out}" 2>&1 &
    child_pid=$!
  fi
  (
    sleep "${timeout_secs}"
    if kill -0 "${child_pid}" 2>/dev/null; then
      : >"${timeout_flag}"
      if [ "${use_pgroup}" = "true" ]; then
        kill -TERM "-${child_pid}" 2>/dev/null || true
      else
        kill -TERM "${child_pid}" 2>/dev/null || true
      fi
      sleep 2
      if kill -0 "${child_pid}" 2>/dev/null; then
        if [ "${use_pgroup}" = "true" ]; then
          kill -KILL "-${child_pid}" 2>/dev/null || true
        else
          kill -KILL "${child_pid}" 2>/dev/null || true
        fi
      fi
    fi
  ) &
  watcher_pid=$!
  wait "${child_pid}"
  rc=$?
  kill "${watcher_pid}" 2>/dev/null || true
  cat "${tmp_out}"
  rm -f "${tmp_out}"
  if [ -f "${timeout_flag}" ]; then
    rm -f "${timeout_flag}"
    return 124
  fi
  rm -f "${timeout_flag}"
  return "${rc}"
}
set +e
FIX_RESULT="$(run_with_hard_timeout 1800 csa run --tool codex --idle-timeout 1800 "Bounded fallback-fix task only. Do NOT invoke pr-codex-bot skill or any full PR workflow. Operate on PR #${PR_NUM} in repo ${REPO}. Bot is unavailable and fallback local review found issues. Run a self-contained max-3-round fix cycle: read latest findings from csa review --range main...HEAD, apply fixes with commits, re-run review, repeat until clean. Return exactly one marker line FALLBACK_FIX=clean when clean; otherwise return FALLBACK_FIX=failed and exit non-zero.")"
FIX_RC=$?
set -e

if [ "${FIX_RC}" -eq 124 ]; then
  echo "ERROR: Fallback fix cycle exceeded hard timeout (1800s)." >&2
  exit 1
fi
if [ "${FIX_RC}" -ne 0 ]; then
  echo "ERROR: Fallback fix cycle failed (rc=${FIX_RC})." >&2
  exit 1
fi
FIX_MARKER="$(
  printf '%s\n' "${FIX_RESULT}" \
    | grep -E '^[[:space:]]*FALLBACK_FIX=(clean|failed)[[:space:]]*$' \
    | tail -n 1 \
    | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//' \
    || true
)"
if [ "${FIX_MARKER}" != "FALLBACK_FIX=clean" ]; then
  echo "ERROR: Fallback fix cycle returned invalid marker." >&2
  exit 1
fi

FALLBACK_REVIEW_HAS_ISSUES=false
echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=${FALLBACK_REVIEW_HAS_ISSUES}"
```

## Step 6a: Merge Without Bot

> **Layer**: 0 (Orchestrator) -- merge command, no code analysis.

Tool: bash

Bot unavailable. Local fallback review passed (either initially in Step 5,
or after fix cycle in Step 6-fix). Step 6-fix guarantees
`FALLBACK_REVIEW_HAS_ISSUES=false` before reaching this point.

**MANDATORY**: Before merging, leave a PR comment explaining the merge rationale
(bot timeout + local review CLEAN). This provides audit trail for reviewers.

```bash
# --- Hard gate: unconditional pre-merge check ---
if [ "${FALLBACK_REVIEW_HAS_ISSUES:-false}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved fallback review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi
if [ "${REBASE_REVIEW_HAS_ISSUES:-false}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved post-rebase review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi

# Push fallback fix commits so the remote PR head includes them.
# Without this, gh pr merge uses the stale remote HEAD and omits fixes.
git push origin "${WORKFLOW_BRANCH}"

# Audit trail: explain why merging without bot review.
gh pr comment "${PR_NUM}" --repo "${REPO}" --body \
  "**Merge rationale**: Cloud bot (@codex) is disabled or unavailable. Local \`csa review --branch main\` passed CLEAN (or issues were fixed in fallback cycle). Proceeding to merge with local review as the review layer."

gh pr merge "${PR_NUM}" --repo "${REPO}" --squash --delete-branch
git checkout main && git pull origin main
```

## ELSE

## IF ${BOT_HAS_ISSUES}

## Step 7: Evaluate Each Bot Comment

> **Layer**: 1 (claude-code / Task tool) -- Layer 0 dispatches comment
> classification to a sub-agent. The sub-agent reads PR comments and code
> context to classify each one. Orchestrator uses classifications to route
> to Step 8 (debate) or Step 9 (fix).

Tool: claude-code
Tier: tier-3-complex

## FOR comment IN ${BOT_COMMENTS}

Classify each comment:
- Category A (already fixed): react and acknowledge
- Category B (suspected false positive): queue for arbitration
- Category C (real issue): queue for fix

## Step 7a: Staleness Filter

Tool: bash
OnFail: skip

For each bot comment, check whether the referenced code has been modified
since the comment was posted. Compare the comment's file paths and line
ranges against the latest HEAD diff (`git diff main...HEAD`) and commit
timestamps (`git log --since`). Comments that reference lines/hunks
modified after the comment timestamp are marked as "potentially stale"
(`COMMENT_IS_STALE=true`) and reclassified as Category A (already
addressed). Stale comments are skipped before entering the debate
arbitration step, preventing wasted cycles debating already-fixed issues.

```bash
# For each comment in BOT_COMMENTS:
#   1. Extract file path and line range from comment body
#   2. Get comment creation timestamp from GitHub API
#   3. Check: git log --since="${COMMENT_TIMESTAMP}" --oneline -- "${FILE}"
#   4. If file changed after comment → COMMENT_IS_STALE=true
#   5. Stale comments are reclassified as Category A (skip arbitration)
```

## IF ${COMMENT_IS_FALSE_POSITIVE} && !(${COMMENT_IS_STALE})

## Step 8: Arbitrate via Debate

> **Layer**: 1 (CSA debate) -- Layer 0 dispatches to `csa debate`, which
> internally spawns Layer 2 independent models for adversarial evaluation.
> Orchestrator receives the verdict and posts audit trail to PR.

Tool: csa
Tier: tier-2-standard

## INCLUDE debate

MUST use independent model for arbitration.
NEVER dismiss bot comments using own reasoning alone.
Post full audit trail (model specs for both sides) to PR.

```bash
csa debate "A code reviewer flagged: ${COMMENT_TEXT}. Evaluate independently."
```

## ELSE

<!-- COMMENT_IS_STALE check is enforced via step conditions in workflow.toml -->

## Step 9: Fix Real Issue

> **Layer**: 1 (CSA executor) -- Layer 0 dispatches fix to CSA sub-agent.
> CSA reads code, applies fix, commits. Orchestrator verifies result.

Tool: csa
Tier: tier-2-standard
OnFail: retry 2

Fix the real issue (non-stale, non-false-positive). Commit the fix.

## ENDIF

## ENDFOR

## Step 10: Push Fixes and Continue Loop

> **Layer**: 0 (Orchestrator) -- shell commands to push fixes and continue loop.

Tool: bash

Track iteration count via `REVIEW_ROUND`. Check the round cap BEFORE
pushing fixes and continuing to the next review loop iteration. When `REVIEW_ROUND`
reaches `MAX_REVIEW_ROUNDS` (default: 10), STOP and present options to
the user — no new review is triggered until the user decides:

- **Option A**: Merge now (review is good enough)
- **Option B**: Continue for `MAX_REVIEW_ROUNDS` more rounds
- **Option C**: Abort and investigate manually

The workflow MUST NOT auto-merge or auto-abort at the round limit.
The user MUST explicitly choose an option before proceeding.

**Orchestrator protocol**: When the round cap is hit, the bash block exits
with code 0 after printing `ROUND_LIMIT_HALT`. The orchestrator (Layer 0)
MUST then use `AskUserQuestion` to present options A/B/C and collect the
user's choice. Based on the answer, set `ROUND_LIMIT_ACTION` and re-enter
this step. The action handler at the TOP of the script processes the user's
choice BEFORE the round cap check, so the chosen action always takes effect:
- **A**: Set `ROUND_LIMIT_ACTION=merge` → clears `ROUND_LIMIT_REACHED`, prints `ROUND_LIMIT_MERGE`, exits 0. Orchestrator routes to Step 12/12b.
- **B**: Set `ROUND_LIMIT_ACTION=continue` → clears `ROUND_LIMIT_REACHED`, extends `MAX_REVIEW_ROUNDS`, falls through to push loop.
- **C**: Set `ROUND_LIMIT_ACTION=abort` → leaves `ROUND_LIMIT_REACHED=true`, prints `ROUND_LIMIT_ABORT`, exits 1.

**CRITICAL**: The `merge` and `continue` branches MUST clear `ROUND_LIMIT_REACHED=false`
before proceeding. Steps 10.5, 11, and 12 are gated by `!(${ROUND_LIMIT_REACHED})`,
so a stale `true` value blocks all downstream merge/rebase paths even after the user
explicitly chose to proceed. The `abort` branch intentionally leaves the flag set,
as it halts the workflow.

**Signal disambiguation**: The orchestrator distinguishes re-entry outcomes by
output markers, NOT exit codes alone. `ROUND_LIMIT_HALT` (exit 0) = ask user.
`ROUND_LIMIT_MERGE` (exit 0) = proceed to merge. `ROUND_LIMIT_ABORT` (exit 1) = stop.

```bash
REVIEW_ROUND=$((REVIEW_ROUND + 1))
MAX_REVIEW_ROUNDS="${MAX_REVIEW_ROUNDS:-10}"
echo "CSA_VAR:REVIEW_ROUND=${REVIEW_ROUND}"
echo "CSA_VAR:MAX_REVIEW_ROUNDS=${MAX_REVIEW_ROUNDS}"

# --- Handle orchestrator re-entry with user decision (FIRST) ---
# When the orchestrator re-enters after collecting user choice via
# AskUserQuestion, ROUND_LIMIT_ACTION is set. Process it BEFORE the round
# cap check so the user's choice always takes effect regardless of round count.
#
# CRITICAL: The merge path prints ROUND_LIMIT_MERGE (distinct from
# ROUND_LIMIT_HALT) so the orchestrator can unambiguously route to Step 12/12b.
# The abort path exits non-zero. The continue path falls through to push loop.
if [ -n "${ROUND_LIMIT_ACTION}" ]; then
  case "${ROUND_LIMIT_ACTION}" in
    merge)
      echo "User chose: Merge now. Pushing local commits before merge."
      ROUND_LIMIT_REACHED=false  # Clear so Steps 10.5/11/12 are unblocked
      # Push any Category C fixes from Step 9 so remote HEAD includes them.
      # Without this, gh pr merge merges the stale remote head.
      git push origin "${WORKFLOW_BRANCH}"
      echo "CSA_VAR:ROUND_LIMIT_REACHED=${ROUND_LIMIT_REACHED}"
      echo "CSA_VAR:ROUND_LIMIT_ACTION="
      echo "ROUND_LIMIT_MERGE: Routing to merge step."
      # Orchestrator MUST route to Step 12/12b upon seeing ROUND_LIMIT_MERGE.
      # Distinct from ROUND_LIMIT_HALT — this is an affirmative merge decision.
      exit 0
      ;;
    continue)
      echo "User chose: Continue. Extending by ${MAX_REVIEW_ROUNDS} rounds."
      ROUND_LIMIT_REACHED=false  # Clear so review loop and downstream steps are unblocked
      MAX_REVIEW_ROUNDS=$((REVIEW_ROUND + MAX_REVIEW_ROUNDS))
      unset ROUND_LIMIT_ACTION
      echo "CSA_VAR:ROUND_LIMIT_REACHED=${ROUND_LIMIT_REACHED}"
      echo "CSA_VAR:MAX_REVIEW_ROUNDS=${MAX_REVIEW_ROUNDS}"
      echo "CSA_VAR:ROUND_LIMIT_ACTION="
      # Fall through to push loop below (bypasses round cap check)
      ;;
    abort)
      echo "User chose: Abort workflow."
      echo "ROUND_LIMIT_ABORT: Workflow terminated by user."
      exit 1
      ;;
  esac
fi

# --- Round cap check BEFORE push/next-loop ---
# This block ONLY fires when ROUND_LIMIT_ACTION is unset (first hit, or after
# continue already extended the cap). When ROUND_LIMIT_ACTION was set, the case
# block above already handled it and either exited or fell through past this check.
if [ "${REVIEW_ROUND}" -ge "${MAX_REVIEW_ROUNDS}" ]; then
  ROUND_LIMIT_REACHED=true
  echo "Reached MAX_REVIEW_ROUNDS (${MAX_REVIEW_ROUNDS})."
  echo "Options:"
  echo "  A) Merge now (review is good enough)"
  echo "  B) Continue for ${MAX_REVIEW_ROUNDS} more rounds"
  echo "  C) Abort and investigate manually"
  echo ""
  echo "CSA_VAR:ROUND_LIMIT_REACHED=${ROUND_LIMIT_REACHED}"
  echo "ROUND_LIMIT_HALT: Awaiting user decision."
  # HALT: The orchestrator MUST use AskUserQuestion to collect user's choice.
  # The shell script block ENDS here. The orchestrator handles routing based on
  # the user's answer OUTSIDE this script block. This ensures non-interactive
  # execution environments (CSA sub-agents) do not hang on stdin.
  #
  # Orchestrator routing logic (executed at Layer 0, NOT in this bash block):
  #   User answers "A" → set ROUND_LIMIT_ACTION=merge, re-enter this step
  #   User answers "B" → set ROUND_LIMIT_ACTION=continue, re-enter this step
  #   User answers "C" → set ROUND_LIMIT_ACTION=abort, re-enter this step
  #
  # FORBIDDEN: Falling through to push loop without a user decision.
  exit 0  # Yield control to orchestrator for AskUserQuestion
fi

# --- Push fixes only (next trigger happens in Step 5) ---
git push origin "${WORKFLOW_BRANCH}"
ROUND_LIMIT_REACHED=false
echo "CSA_VAR:ROUND_LIMIT_REACHED=${ROUND_LIMIT_REACHED}"
echo "CSA_VAR:REVIEW_ROUND=${REVIEW_ROUND}"
echo "CSA_VAR:MAX_REVIEW_ROUNDS=${MAX_REVIEW_ROUNDS}"
```

Loop back to Step 5 (delegated wait gate).

## ELSE

## Step 10a: Bot Review Clean

No issues found by bot. Proceed to Step 10.5 (rebase) then merge.

## Step 10.5: Rebase for Clean History

> **Layer**: 0 (Orchestrator) -- git history cleanup before merge.

Tool: bash

Reorganize accumulated fix commits into logical groups (source, patterns, other)
before merging. Skip if <= 3 commits.

After rebase: backup branch, soft reset to merge-base, dynamic file grouping,
force-push with lease, trigger final `@codex review`, then delegate the long
wait/fix/review loop to a single CSA-managed step.

**Post-rebase review gate** (BLOCKING):
- CSA delegated step handles both paths:
  - Bot responds with P0/P1/P2 badges → CSA runs bounded fix/review retries (max 3 rounds).
  - Bot times out → CSA runs fallback `csa review --range main...HEAD` and bounded fix/review retries (max 3 rounds).
- command-level hard timeout is enforced for the delegated gate (`timeout 2400s`).
- if `timeout/gtimeout` is unavailable, a built-in watchdog fallback still enforces the same timeout bound.
- delegated execution failures are hard failures (no `|| true` silent downgrade).
- On delegated gate failure (timeout, non-zero, or non-PASS marker), set `REBASE_REVIEW_HAS_ISSUES=true` (and `FALLBACK_REVIEW_HAS_ISSUES=true` when appropriate), then block merge.
- On success, both `REBASE_REVIEW_HAS_ISSUES` and `FALLBACK_REVIEW_HAS_ISSUES` must be false.

```bash
set -euo pipefail
TIMEOUT_BIN="$(command -v timeout || command -v gtimeout || true)"
run_with_hard_timeout() {
  local timeout_secs="$1"
  shift
  if [ -n "${TIMEOUT_BIN}" ]; then
    "${TIMEOUT_BIN}" -k 5s "${timeout_secs}s" "$@" 2>&1
    return $?
  fi

  local tmp_out timeout_flag child_pid watcher_pid rc use_pgroup
  tmp_out="$(mktemp)"
  timeout_flag="$(mktemp)"
  rm -f "${timeout_flag}"
  use_pgroup=false
  if command -v setsid >/dev/null 2>&1; then
    setsid "$@" >"${tmp_out}" 2>&1 &
    use_pgroup=true
    child_pid=$!
  else
    "$@" >"${tmp_out}" 2>&1 &
    child_pid=$!
  fi
  (
    sleep "${timeout_secs}"
    if kill -0 "${child_pid}" 2>/dev/null; then
      : >"${timeout_flag}"
      if [ "${use_pgroup}" = "true" ]; then
        kill -TERM "-${child_pid}" 2>/dev/null || true
      else
        kill -TERM "${child_pid}" 2>/dev/null || true
      fi
      sleep 2
      if kill -0 "${child_pid}" 2>/dev/null; then
        if [ "${use_pgroup}" = "true" ]; then
          kill -KILL "-${child_pid}" 2>/dev/null || true
        else
          kill -KILL "${child_pid}" 2>/dev/null || true
        fi
      fi
    fi
  ) &
  watcher_pid=$!
  wait "${child_pid}"
  rc=$?
  kill "${watcher_pid}" 2>/dev/null || true
  cat "${tmp_out}"
  rm -f "${tmp_out}"
  if [ -f "${timeout_flag}" ]; then
    rm -f "${timeout_flag}"
    return 124
  fi
  rm -f "${timeout_flag}"
  return "${rc}"
}

COMMIT_COUNT=$(git rev-list --count main..HEAD)
if [ "${COMMIT_COUNT}" -gt 3 ]; then
  git branch -f "backup-${PR_NUM}-pre-rebase" HEAD

  MERGE_BASE=$(git merge-base main HEAD)
  git reset --soft $MERGE_BASE

  git reset HEAD .
  git diff --name-only -z HEAD | { grep -zE '^(src/|crates/|lib/|bin/)' || true; } | xargs -0 --no-run-if-empty git add --
  if ! git diff --cached --quiet; then
    git commit -m "feat(scope): primary implementation changes"
  fi

  git diff --name-only -z HEAD | { grep -zE '^(patterns/|\.claude/)' || true; } | xargs -0 --no-run-if-empty git add --
  if ! git diff --cached --quiet; then
    git commit -m "fix(scope): pattern and skill updates"
  fi

  git add -A
  if ! git diff --cached --quiet; then
    git commit -m "chore(scope): config and documentation updates"
  fi

  NEW_COMMIT_COUNT=$(git rev-list --count ${MERGE_BASE}..HEAD)
  if [ "${NEW_COMMIT_COUNT}" -eq 0 ]; then
    echo "ERROR: No replacement commits after soft reset. Restoring backup."
    git reset --hard "backup-${PR_NUM}-pre-rebase"
    exit 1
  fi

  git push --force-with-lease
  gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review"

  set +e
  GATE_RESULT="$(run_with_hard_timeout 2400 csa run --tool codex --idle-timeout 2400 "Bounded post-rebase gate task only. Do NOT invoke pr-codex-bot skill or any full PR workflow. Operate on PR #${PR_NUM} in repo ${REPO} (branch ${WORKFLOW_BRANCH}). Complete the post-rebase review gate end-to-end: wait up to 10 minutes for @codex response to the latest trigger; if response contains P0/P1/P2 findings, iteratively fix/commit/push/re-trigger and re-check (max 3 rounds); if bot times out, run csa review --range main...HEAD and execute a max-3-round fix/review cycle; leave an audit-trail PR comment whenever timeout fallback path is used; return exactly one marker line REBASE_GATE=PASS when clean, otherwise REBASE_GATE=FAIL and exit non-zero.")"
  GATE_RC=$?
  set -e
  if [ "${GATE_RC}" -eq 124 ]; then
    REBASE_REVIEW_HAS_ISSUES=true
    FALLBACK_REVIEW_HAS_ISSUES=true
    echo "CSA_VAR:REBASE_REVIEW_HAS_ISSUES=${REBASE_REVIEW_HAS_ISSUES}"
    echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=${FALLBACK_REVIEW_HAS_ISSUES}"
    echo "ERROR: Post-rebase delegated gate exceeded hard timeout (2400s)." >&2
    exit 1
  fi
  if [ "${GATE_RC}" -ne 0 ]; then
    REBASE_REVIEW_HAS_ISSUES=true
    FALLBACK_REVIEW_HAS_ISSUES=true
    echo "CSA_VAR:REBASE_REVIEW_HAS_ISSUES=${REBASE_REVIEW_HAS_ISSUES}"
    echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=${FALLBACK_REVIEW_HAS_ISSUES}"
    echo "ERROR: Post-rebase delegated gate failed (rc=${GATE_RC})." >&2
    exit 1
  fi

  GATE_MARKER="$(
    printf '%s\n' "${GATE_RESULT}" \
      | grep -E '^[[:space:]]*REBASE_GATE=(PASS|FAIL)[[:space:]]*$' \
      | tail -n 1 \
      | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//' \
      || true
  )"
  if [ "${GATE_MARKER}" != "REBASE_GATE=PASS" ]; then
    REBASE_REVIEW_HAS_ISSUES=true
    FALLBACK_REVIEW_HAS_ISSUES=true
    echo "CSA_VAR:REBASE_REVIEW_HAS_ISSUES=${REBASE_REVIEW_HAS_ISSUES}"
    echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=${FALLBACK_REVIEW_HAS_ISSUES}"
    echo "ERROR: Post-rebase review gate failed."
    exit 1
  fi

  REBASE_REVIEW_HAS_ISSUES=false
  FALLBACK_REVIEW_HAS_ISSUES=false
  echo "CSA_VAR:REBASE_REVIEW_HAS_ISSUES=${REBASE_REVIEW_HAS_ISSUES}"
  echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=${FALLBACK_REVIEW_HAS_ISSUES}"
  git push origin "${WORKFLOW_BRANCH}"
fi
```

## ENDIF

## ENDIF

## ENDIF
<!-- End of CLOUD_BOT != "false" block -->

## IF !(${BOT_UNAVAILABLE})

## IF ${FIXES_ACCUMULATED}

## Step 11: Clean Resubmission (if needed)

> **Layer**: 0 (Orchestrator) -- git branch management, no code reading.

Tool: bash

If fixes accumulated, create clean branch for final review.

```bash
CLEAN_BRANCH="${WORKFLOW_BRANCH}-clean"
git checkout -b "${CLEAN_BRANCH}"
git push -u origin "${CLEAN_BRANCH}"
gh pr create --base main --head "${CLEAN_BRANCH}" --title "${PR_TITLE}" --body "${PR_BODY}"
```

## Step 12: Final Merge

> **Layer**: 0 (Orchestrator) -- final merge command, no code analysis.

Tool: bash
OnFail: abort

Squash-merge and update local main.

```bash
# --- Hard gate: unconditional pre-merge check ---
if [ "${FALLBACK_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved fallback review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi
if [ "${REBASE_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved post-rebase review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi

git push origin "${WORKFLOW_BRANCH}"
gh pr merge "${WORKFLOW_BRANCH}-clean" --repo "${REPO}" --squash --delete-branch
git checkout main && git pull origin main
```

## ELSE

## Step 12b: Final Merge (Direct)

> **Layer**: 0 (Orchestrator) -- direct merge, no code analysis needed.

Tool: bash
OnFail: abort

First-pass clean review: merge the existing PR directly.

```bash
# --- Hard gate: unconditional pre-merge check ---
if [ "${FALLBACK_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved fallback review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi
if [ "${REBASE_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved post-rebase review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi

git push origin "${WORKFLOW_BRANCH}"
gh pr merge "${PR_NUM}" --repo "${REPO}" --squash --delete-branch
git checkout main && git pull origin main
```

## ENDIF

## ENDIF
