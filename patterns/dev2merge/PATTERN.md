---
name = "dev2merge"
description = "Deterministic development pipeline: branch validation, planning, N*(implement+commit), pre-PR review, push, PR, codex-bot merge"
allowed-tools = "Bash, Read, Edit, Write, Grep, Glob, Task, TaskCreate, TaskUpdate, TaskList, TaskGet"
tier = "tier-3-complex"
version = "0.2.0"
---

# Dev2Merge: Deterministic Development Pipeline

End-to-end development workflow enforced as a weave workflow. Every stage has
hard gates (`on_fail = "abort"`). No step can be skipped by the LLM.

Pipeline: Branch Validation → FAST_PATH Detection → mktd (planning) →
mktsk N*(implement → commit) → Pre-PR Cumulative Review → Push → PR →
pr-codex-bot (review loop + merge) → Local Sync.

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
  MKTD_TOOL_ARGS=(--tool "${MKTD_TOOL_EFFECTIVE}")
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

Tool: bash
OnFail: abort

Bridge planning to execution. mktsk processes TODO items serially.
Each item goes through implement → commit (via INCLUDE commit).

```bash
set -euo pipefail
timeout 3600 csa run --sa-mode true --skill mktsk "${MKTD_TODO_TIMESTAMP}"
```

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

## Step 12: Create Pull Request Transaction

Tool: bash
OnFail: abort

Create or reuse PR, then synchronously run the post-create helper.
This makes PR creation + pr-codex-bot a single shell-enforced transaction.

```bash
set -euo pipefail
BRANCH="$(git branch --show-current)"
COMMIT_SUBJECT="$(git log -1 --format=%s)"
set +e
CREATE_OUTPUT="$(gh pr create --base main --title "${COMMIT_SUBJECT}" --body "Auto-created by dev2merge pipeline." 2>&1)"
CREATE_RC=$?
set -e
if [ "${CREATE_RC}" -ne 0 ]; then
  if ! printf '%s\n' "${CREATE_OUTPUT}" | grep -Eiq 'already exists|a pull request already exists'; then
    echo "ERROR: gh pr create failed: ${CREATE_OUTPUT}" >&2
    exit 1
  fi
  echo "INFO: PR already exists for ${BRANCH}; continuing with post-create helper."
fi
scripts/hooks/post-pr-create.sh --base main
```

## Step 13: Post-Merge Local Sync

Tool: bash
OnFail: abort

After pr-codex-bot merges, sync local main and clean up feature branch.

```bash
set -euo pipefail
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
if [ -n "${FEATURE_BRANCH}" ] && [ "${FEATURE_BRANCH}" != "main" ] && [ "${FEATURE_BRANCH}" != "dev" ]; then
  git branch -d "${FEATURE_BRANCH}" 2>/dev/null || echo "INFO: Local branch ${FEATURE_BRANCH} already deleted."
fi
```
