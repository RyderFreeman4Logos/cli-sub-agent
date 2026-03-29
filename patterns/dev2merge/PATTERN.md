---
name = "dev2merge"
description = "Deterministic development pipeline: branch validation, planning, N*(implement+commit), pre-PR review, push, PR, codex-bot merge"
allowed-tools = "Bash, Read, Edit, Write, Grep, Glob, Task, TaskCreate, TaskUpdate, TaskList, TaskGet"
tier = "tier-3-complex"
version = "0.4.0"
---

# Dev2Merge: Deterministic Development Pipeline

End-to-end development workflow enforced as a weave workflow. Every stage has
hard gates (`on_fail = "abort"`). No step can be skipped by the LLM.

Pipeline: Branch Validation → FAST_PATH Detection → mktd (planning) →
mktsk N*(implement → commit) → Pre-PR Cumulative Review → Push →
PR Creation → pr-bot Hard Gate → Post-Merge Sync.

Sub-workflows are included via `## INCLUDE`, not inlined.

## Step 1: Validate Branch

Tool: bash
OnFail: abort

Verify the current branch is a feature branch, not protected.

```bash
BRANCH="$(git branch --show-current)"
if [ -z "${BRANCH}" ] || [ "${BRANCH}" = "HEAD" ]; then
  echo "ERROR: Cannot determine current branch."
  exit 1
fi
DEFAULT_BRANCH=$(git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's@^refs/remotes/origin/@@')
if [ -z "$DEFAULT_BRANCH" ]; then DEFAULT_BRANCH="main"; fi
if [ "$BRANCH" = "$DEFAULT_BRANCH" ] || [ "$BRANCH" = "dev" ]; then
  echo "ERROR: Cannot work directly on $BRANCH. Create a feature branch."
  exit 1
fi
echo "CSA_VAR:WORKFLOW_BRANCH=$BRANCH"
echo "CSA_VAR:DEFAULT_BRANCH=$DEFAULT_BRANCH"
```

## Step 2: FAST_PATH Detection

Tool: bash
OnFail: abort

Detect whether changes are docs/config-only. When FAST_PATH=true,
skip mktd/mktsk/debate but keep L1/L2 quality checks.

```bash
set -euo pipefail
CODE_FILES="$(git diff --name-only "${DEFAULT_BRANCH}...HEAD" 2>/dev/null | grep -cvE '\.(md|txt|lock|toml)$' || true)"
TOTAL_INSERTIONS="$(git diff --stat "${DEFAULT_BRANCH}...HEAD" 2>/dev/null | tail -1 | grep -oE '[0-9]+ insertion' | grep -oE '[0-9]+' || echo 0)"
if [ "${CODE_FILES}" -eq 0 ] && [ "${TOTAL_INSERTIONS:-0}" -lt 100 ]; then
  echo "FAST_PATH: docs/config-only changes detected. Skipping mktd/mktsk."
  echo "CSA_VAR:FAST_PATH=true"
else
  echo "Full pipeline: ${CODE_FILES} code files, ${TOTAL_INSERTIONS} insertions."
  echo "CSA_VAR:FAST_PATH=false"
fi
```

## Step 3: L1/L2 Quality Gates (Always Run)

Tool: bash
OnFail: abort

Formatters and linters run regardless of FAST_PATH.

```bash
just fmt
just clippy
```

## IF ${FAST_PATH}

## Step 4: FAST_PATH Commit

Tool: bash
OnFail: abort

For docs/config-only changes, run a simplified commit flow:
stage, generate message, commit. No mktd/mktsk/security-audit overhead.

```bash
set -euo pipefail
just test
git add -A
if ! git diff --cached --name-only | grep -q .; then
  echo "ERROR: No staged files."
  exit 1
fi
COMMIT_MSG="$(scripts/gen_commit_msg.sh "${SCOPE:-}" 2>/dev/null || echo "docs: update documentation")"
git commit -m "${COMMIT_MSG}"
echo "CSA_VAR:FAST_PATH_COMMITTED=true"
```

## Step 5: FAST_PATH Version Bump

Tool: bash
OnFail: abort

```bash
set -euo pipefail
if ! just check-version-bumped 2>/dev/null; then
  just bump-patch
  cargo run -p weave -- lock 2>/dev/null || true
  git add Cargo.toml Cargo.lock weave.lock 2>/dev/null || git add Cargo.toml weave.lock
  VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "cli-sub-agent") | .version')"
  git commit -m "chore(release): bump workspace version to ${VERSION}"
fi
```

## Step 6: FAST_PATH Pre-PR Review

Tool: bash
OnFail: abort

Even FAST_PATH runs cumulative review before push.

```bash
set -euo pipefail
REVIEW_OUTPUT="$(timeout 1800 csa review --sa-mode true --range "${DEFAULT_BRANCH}...HEAD" 2>&1)" || true
printf '%s\n' "${REVIEW_OUTPUT}"
echo "CSA_VAR:REVIEW_COMPLETED=true"
```

## ELSE

## Step 7: Plan with mktd

Tool: bash
OnFail: abort

Generate a TODO plan via mktd. Includes debate phase.

```bash
set -euo pipefail
CURRENT_BRANCH="$(git branch --show-current)"
FEATURE_INPUT="${SCOPE:-current branch changes pending merge}"
USER_LANGUAGE_OVERRIDE="${CSA_USER_LANGUAGE:-}"
MKTD_TOOL_EFFECTIVE="${MKTD_TOOL:-${CSA_MKTD_TOOL:-gemini-cli}}"
if [ -n "${MKTD_TOOL_EFFECTIVE}" ]; then
  MKTD_TOOL_ARGS=(--force-ignore-tier-setting --tool "${MKTD_TOOL_EFFECTIVE}")
else
  MKTD_TOOL_ARGS=()
fi
timeout 1800 csa plan run patterns/mktd/workflow.toml \
  "${MKTD_TOOL_ARGS[@]}" \
  --var CWD="$(pwd)" \
  --var FEATURE="Plan dev2merge for branch ${CURRENT_BRANCH}. Scope: ${FEATURE_INPUT}." \
  --var USER_LANGUAGE="${USER_LANGUAGE_OVERRIDE}"
LATEST_TS="$(csa todo list --format json | jq -r --arg br "${CURRENT_BRANCH}" '[.[] | select(.branch == $br)] | sort_by(.timestamp) | last | .timestamp // empty')"
if [ -z "${LATEST_TS}" ]; then
  echo "ERROR: mktd did not produce a TODO for branch ${CURRENT_BRANCH}." >&2
  exit 1
fi
TODO_PATH="$(csa todo show -t "${LATEST_TS}" --path)"
grep -qF -- '- [ ] ' "${TODO_PATH}" || { echo "ERROR: TODO missing checkbox tasks." >&2; exit 1; }
grep -q 'DONE WHEN' "${TODO_PATH}" || { echo "ERROR: TODO missing DONE WHEN clauses." >&2; exit 1; }
echo "CSA_VAR:MKTD_TODO_TIMESTAMP=${LATEST_TS}"
echo "CSA_VAR:MKTD_TODO_PATH=${TODO_PATH}"
```

## Step 8: Execute Plan with mktsk

OnFail: abort

Invoke the mktsk skill directly (NOT via `csa run`). mktsk MUST run in the
main agent context so it can use TaskCreate/TaskUpdate for progress tracking.

Pass the TODO timestamp: `${MKTD_TODO_TIMESTAMP}`
Set env: `CSA_SKIP_PUBLISH=true` (dev2merge handles publish in Steps 12-13).

mktsk reads the TODO plan, registers tasks via TaskCreate, and executes each
item serially: implement → quality gates → review → commit → next.

## Step 9: Ensure Version Bumped

Tool: bash
OnFail: abort

```bash
set -euo pipefail
if ! just check-version-bumped 2>/dev/null; then
  just bump-patch
  cargo run -p weave -- lock 2>/dev/null || true
  git add Cargo.toml Cargo.lock weave.lock 2>/dev/null || git add Cargo.toml weave.lock
  VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "cli-sub-agent") | .version')"
  git commit -m "chore(release): bump workspace version to ${VERSION}"
fi
```

## Step 10: Pre-PR Cumulative Review Gate

Tool: bash
OnFail: abort

Cumulative review covering all commits since main.
Sets REVIEW_COMPLETED=true as gate for push step.

```bash
set -euo pipefail
REVIEW_OUTPUT_FILE="$(mktemp)"
set +e
timeout 1800 csa review --sa-mode true --range "${DEFAULT_BRANCH}...HEAD" 2>&1 | tee "${REVIEW_OUTPUT_FILE}"
REVIEW_STATUS=${PIPESTATUS[0]}
set -e
REVIEW_OUTPUT="$(cat "${REVIEW_OUTPUT_FILE}")"
rm -f "${REVIEW_OUTPUT_FILE}"
if [ "${REVIEW_STATUS}" -eq 124 ]; then
  echo "ERROR: Cumulative review timed out." >&2
  exit 1
fi
VERDICT_LINE="$(printf '%s\n' "${REVIEW_OUTPUT}" | grep '^final_decision:' | tail -1 || true)"
if [ "${VERDICT_LINE}" = "final_decision: HAS_ISSUES" ]; then
  echo "ERROR: Cumulative review found issues. Cannot push." >&2
  exit 1
fi
echo "CSA_VAR:REVIEW_COMPLETED=true"
```

## ENDIF

## Step 11: Push Gate

Tool: bash
OnFail: abort

Hard gate: REVIEW_COMPLETED must be true before any push.

```bash
if [ "${REVIEW_COMPLETED:-}" != "true" ]; then
  echo "ERROR: Push blocked — pre-PR review not completed."
  echo "REVIEW_COMPLETED=${REVIEW_COMPLETED:-unset}"
  exit 1
fi
BRANCH="$(git branch --show-current)"
git push -u origin "${BRANCH}" --force-with-lease
echo "CSA_VAR:PUSHED=true"
```

## Step 12: Create or Reuse Pull Request

Tool: bash
OnFail: abort

Create or reuse a PR for the current branch. Outputs PR_NUMBER and PR_URL
as CSA_VARs for the next step. This step does NOT trigger pr-bot —
that is a separate hard gate in Step 13.

```bash
set -euo pipefail
BRANCH="$(git branch --show-current)"
COMMIT_SUBJECT="$(git log -1 --format=%s)"

# --- Create or reuse PR ---
set +e
CREATE_OUTPUT="$(gh pr create --base main --title "${COMMIT_SUBJECT}" --body "Auto-created by dev2merge pipeline." 2>&1)"
CREATE_RC=$?
set -e
if [ "${CREATE_RC}" -ne 0 ]; then
  if ! printf '%s\n' "${CREATE_OUTPUT}" | grep -Eiq 'already exists|a pull request already exists'; then
    echo "ERROR: gh pr create failed: ${CREATE_OUTPUT}" >&2
    exit 1
  fi
  echo "INFO: PR already exists for ${BRANCH}; continuing."
fi

# --- Resolve PR number (retry + fallback) ---
PR_NUMBER=""
PR_URL=""
for attempt in 1 2 3; do
  PR_JSON="$(gh pr view --json number,url,state -q 'select(.state == "OPEN")' 2>/dev/null || true)"
  PR_NUMBER="$(printf '%s' "${PR_JSON}" | jq -r '.number // empty' 2>/dev/null || true)"
  if [ -n "${PR_NUMBER}" ] && printf '%s' "${PR_NUMBER}" | grep -Eq '^[0-9]+$'; then
    PR_URL="$(printf '%s' "${PR_JSON}" | jq -r '.url // empty')"
    break
  fi
  PR_NUMBER=""
  if [ "${attempt}" -lt 3 ]; then
    echo "INFO: PR resolution attempt ${attempt} failed, retrying in 5s..."
    sleep 5
  fi
done
# Fallback: gh pr list --head
if [ -z "${PR_NUMBER}" ]; then
  echo "INFO: gh pr view failed; falling back to gh pr list..."
  PR_NUMBER="$(gh pr list --head "${BRANCH}" --base main --json number -q '.[0].number' 2>/dev/null || true)"
  if [ -n "${PR_NUMBER}" ] && printf '%s' "${PR_NUMBER}" | grep -Eq '^[0-9]+$'; then
    PR_URL="$(gh pr view "${PR_NUMBER}" --json url -q '.url' 2>/dev/null || true)"
  fi
fi
if [ -z "${PR_NUMBER}" ] || ! printf '%s' "${PR_NUMBER}" | grep -Eq '^[0-9]+$'; then
  echo "ERROR: Cannot resolve open PR number for ${BRANCH} after retries." >&2
  exit 1
fi
echo "PR #${PR_NUMBER} resolved: ${PR_URL}"
echo "CSA_VAR:PR_NUMBER=${PR_NUMBER}"
echo "CSA_VAR:PR_URL=${PR_URL}"
```

## Step 13: pr-bot Review & Merge Gate (HARD GATE)

Tool: bash
OnFail: abort

**MANDATORY**: This step MUST NOT be skipped. It runs pr-bot which performs
cloud review (if enabled) and the actual merge. Without this step completing
successfully, the PR remains unmerged and Step 14 will fail.

Uses marker files for idempotency: skips if pr-bot already completed for
the same PR/HEAD combination.

```bash
set -euo pipefail
if [ -z "${PR_NUMBER:-}" ]; then
  echo "ERROR: PR_NUMBER not set — Step 12 must run first." >&2
  exit 1
fi
HEAD_SHA="$(git rev-parse --verify HEAD)"

# --- Lock + Idempotency: skip if pr-bot already ran or is running ---
# Bind markers to repo identity to prevent cross-repo PR# collisions.
REPO_SLUG="$(gh repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null | tr '/' '_')"
if [ -z "${REPO_SLUG}" ]; then
  REPO_SLUG="$(git remote get-url origin 2>/dev/null | sed -E 's#^(https?://[^/]+/|ssh://[^/]+/|[^:]+:)##; s/\.git$//' | tr '/' '_')"
fi
MARKER_DIR="${HOME}/.local/state/cli-sub-agent/pr-bot-markers/${REPO_SLUG}"
mkdir -p "${MARKER_DIR}"
MARKER_BASE="${MARKER_DIR}/${PR_NUMBER}-${HEAD_SHA}"
DONE_MARKER="${MARKER_BASE}.done"
LOCK_DIR="${MARKER_BASE}.lock"
LOCK_HELD=0

cleanup_lock() {
  if [ "${LOCK_HELD}" -eq 1 ]; then
    rmdir "${LOCK_DIR}" 2>/dev/null || true
  fi
}
trap cleanup_lock EXIT

if [ -f "${DONE_MARKER}" ]; then
  echo "pr-bot already completed for PR #${PR_NUMBER} at HEAD ${HEAD_SHA:0:11}; skipping."
  echo "CSA_VAR:PR_BOT_DONE_MARKER=${DONE_MARKER}"
elif ! mkdir "${LOCK_DIR}" 2>/dev/null; then
  echo "ERROR: pr-bot already running for PR #${PR_NUMBER} at HEAD ${HEAD_SHA:0:11}." >&2
  echo "Wait for the other run to finish, or remove the lock: ${LOCK_DIR}" >&2
  exit 1
else
  LOCK_HELD=1
  echo "Running pr-bot for PR #${PR_NUMBER} (${PR_URL:-unknown})..."
  export CSA_PR_BOT_GUARD=1
  if csa plan run --sa-mode true patterns/pr-bot/workflow.toml; then
    touch "${DONE_MARKER}"
    echo "CSA_VAR:PR_BOT_DONE_MARKER=${DONE_MARKER}"
    LOCK_HELD=0
    rmdir "${LOCK_DIR}" 2>/dev/null || true
  else
    echo "ERROR: pr-bot workflow failed for PR #${PR_NUMBER}." >&2
    exit 1
  fi
fi
```

## Step 14: Post-Merge Local Sync

Tool: bash
OnFail: abort

Verify pr-bot completion marker exists (deterministic gate — cannot be bypassed
by LLM executor) AND that the PR was actually merged. Both checks must pass.

```bash
set -euo pipefail
# NOTE: PR_NUMBER comes from Step 12 (gh pr view/list). In fork workflows,
# pr-bot may resolve a different PR via owner-aware lookup. For single-repo
# workflows (the common case), both resolve to the same PR.
if [ -n "${PR_NUMBER:-}" ]; then
  # --- Deterministic gate: verify pr-bot completion marker ---
  # Prefer exact marker path from Step 13 (CSA_VAR:PR_BOT_DONE_MARKER).
  # Fall back to repo-scoped glob if variable is unset (backwards compat).
  if [ -n "${PR_BOT_DONE_MARKER:-}" ]; then
    if [ ! -f "${PR_BOT_DONE_MARKER}" ]; then
      echo "ERROR: pr-bot marker not found: ${PR_BOT_DONE_MARKER}" >&2
      echo "Step 13 (pr-bot) must complete successfully before post-merge sync." >&2
      exit 1
    fi
    echo "pr-bot completion marker verified (exact): ${PR_BOT_DONE_MARKER}"
  else
    # Fallback: glob match by repo slug + PR number.
    # NOTE: glob may match stale markers from previous pr-bot runs on the same
    # PR. The exact CSA_VAR path (above) is the primary defense; this fallback
    # exists only for edge cases where the variable is lost.
    REPO_SLUG="$(gh repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null | tr '/' '_')"
    if [ -z "${REPO_SLUG}" ]; then
      REPO_SLUG="$(git remote get-url origin 2>/dev/null | sed -E 's#^(https?://[^/]+/|ssh://[^/]+/|[^:]+:)##; s/\.git$//' | tr '/' '_')"
    fi
    MARKER_DIR="${HOME}/.local/state/cli-sub-agent/pr-bot-markers/${REPO_SLUG}"
    if ! ls "${MARKER_DIR}/${PR_NUMBER}"-*.done 1>/dev/null 2>&1; then
      echo "ERROR: No pr-bot completion marker found for PR #${PR_NUMBER}." >&2
      echo "Step 13 (pr-bot) must complete successfully before post-merge sync." >&2
      echo "Marker directory: ${MARKER_DIR}" >&2
      exit 1
    fi
    echo "pr-bot completion marker verified (glob) for PR #${PR_NUMBER}."
  fi

  # --- Verify PR is actually merged (defense in depth) ---
  PR_STATE="$(gh pr view "${PR_NUMBER}" --json state -q '.state' 2>/dev/null || echo "UNKNOWN")"
  if [ "${PR_STATE}" != "MERGED" ]; then
    echo "ERROR: PR #${PR_NUMBER} state is '${PR_STATE}', expected 'MERGED'." >&2
    echo "pr-bot marker exists but PR not merged — possible partial failure." >&2
    exit 1
  fi
  echo "PR #${PR_NUMBER} confirmed MERGED."
fi
FEATURE_BRANCH="$(git branch --show-current 2>/dev/null || true)"
git fetch origin
git checkout main
git merge origin/main --ff-only
LOCAL_SHA="$(git rev-parse HEAD)"
REMOTE_SHA="$(git rev-parse origin/main)"
if [ "${LOCAL_SHA}" != "${REMOTE_SHA}" ]; then
  echo "ERROR: Local main does not match origin/main after sync." >&2
  exit 1
fi
echo "Local main synced to ${LOCAL_SHA}."
```
