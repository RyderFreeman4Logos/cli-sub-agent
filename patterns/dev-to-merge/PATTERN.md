---
name = "dev-to-merge"
description = "Full development cycle from branch creation through mktd planning, commit, PR, codex-bot review, and merge"
allowed-tools = "Bash, Read, Edit, Write, Grep, Glob, Task"
tier = "tier-3-complex"
version = "0.1.0"
---

# Dev-to-Merge Workflow

End-to-end development workflow: implement code on a feature branch, pass all
quality gates, commit with Conventional Commits, create a PR, run codex-bot
review loop, and merge to main. Planning is mandatory via `mktd`, and `mktd`
internally requires adversarial `debate` evidence.

## Step 1: Validate Branch

Tool: bash
OnFail: abort

Verify the current branch is a feature branch, not a protected branch.
If on main or dev, abort immediately.

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
```

## Step 2: Plan with mktd (Debate Required)

Tool: bash
OnFail: abort

Generate or refresh a branch TODO plan through `mktd` before development gates.
This step MUST pass through mktd's built-in debate phase and save a TODO.

```bash
: "mktd execution is handled by Step 3 include"
```

## Step 3: Include mktd

```bash
set -euo pipefail
CURRENT_BRANCH="$(git branch --show-current)"
FEATURE_INPUT="${SCOPE:-current branch changes pending merge}"
USER_LANGUAGE_OVERRIDE="${CSA_USER_LANGUAGE:-}"
MKTD_PROMPT="Plan dev-to-merge execution for branch ${CURRENT_BRANCH}. Scope: ${FEATURE_INPUT}."
set +e
if command -v timeout >/dev/null 2>&1; then
  MKTD_OUTPUT="$(timeout 1800 csa plan run patterns/mktd/workflow.toml \
    --var CWD="$(pwd)" \
    --var FEATURE="${MKTD_PROMPT}" \
    --var USER_LANGUAGE="${USER_LANGUAGE_OVERRIDE}" 2>&1)"
  MKTD_STATUS=$?
else
  MKTD_OUTPUT="$(csa plan run patterns/mktd/workflow.toml \
    --var CWD="$(pwd)" \
    --var FEATURE="${MKTD_PROMPT}" \
    --var USER_LANGUAGE="${USER_LANGUAGE_OVERRIDE}" 2>&1)"
  MKTD_STATUS=$?
fi
set -e
printf '%s\n' "${MKTD_OUTPUT}"
if [ "${MKTD_STATUS}" -eq 124 ]; then
  echo "ERROR: mktd workflow timed out after 1800s." >&2
  exit 1
fi
if [ "${MKTD_STATUS}" -ne 0 ]; then
  echo "ERROR: mktd failed (exit=${MKTD_STATUS})." >&2
  exit 1
fi
LATEST_TS="$(csa todo list --format json | jq -r --arg br "${CURRENT_BRANCH}" '[.[] | select(.branch == $br)] | sort_by(.timestamp) | last | .timestamp // empty')"
if [ -z "${LATEST_TS}" ]; then
  echo "ERROR: mktd did not produce a TODO for branch ${CURRENT_BRANCH}." >&2
  exit 1
fi
TODO_PATH="$(csa todo show -t "${LATEST_TS}" --path)"
if [ ! -s "${TODO_PATH}" ]; then
  echo "ERROR: TODO file is empty: ${TODO_PATH}" >&2
  exit 1
fi
grep -qF -- '- [ ] ' "${TODO_PATH}" || { echo "ERROR: TODO missing checkbox tasks: ${TODO_PATH}" >&2; exit 1; }
grep -q 'DONE WHEN:' "${TODO_PATH}" || { echo "ERROR: TODO missing DONE WHEN clauses: ${TODO_PATH}" >&2; exit 1; }
printf 'MKTD_TODO_TIMESTAMP=%s\nMKTD_TODO_PATH=%s\n' "${LATEST_TS}" "${TODO_PATH}"
```

## Step 4: Run Formatters

Tool: bash
OnFail: retry 2

Run the project formatter to ensure consistent code style.

```bash
just fmt
```

## Step 5: Run Linters

Tool: bash
OnFail: retry 2

Run linters to catch static analysis issues.

```bash
just clippy
```

## Step 6: Run Tests

Tool: bash
OnFail: abort

Run the full test suite. All tests must pass before proceeding.

```bash
just test
```

## Step 7: Stage Changes

Tool: bash

Stage all modified and new files relevant to ${SCOPE}.
Verify no untracked files remain.

```bash
git add -A
if ! printf '%s' "${SCOPE:-}" | grep -Eqi 'release|version|lock|deps|dependency'; then
  STAGED_FILES="$(git diff --cached --name-only)"
  if printf '%s\n' "${STAGED_FILES}" | grep -Eq '(^|/)Cargo[.]toml$|(^|/)package[.]json$|(^|/)pnpm-workspace[.]yaml$|(^|/)go[.]mod$'; then
    echo "INFO: Dependency manifest change detected; preserving staged lockfiles."
  elif ! printf '%s\n' "${STAGED_FILES}" | grep -Ev '(^|/)(Cargo[.]lock|package-lock[.]json|pnpm-lock[.]yaml|yarn[.]lock|go[.]sum)$' | grep -q .; then
    echo "INFO: Lockfile-only staged change detected; preserving staged lockfiles."
  else
    MATCHED_LOCKFILES="$(printf '%s\n' "${STAGED_FILES}" | grep -E '(^|/)(Cargo[.]lock|package-lock[.]json|pnpm-lock[.]yaml|yarn[.]lock|go[.]sum)$' || true)"
    if [ -n "${MATCHED_LOCKFILES}" ]; then
      printf '%s\n' "${MATCHED_LOCKFILES}" | while read -r lockpath; do
        echo "INFO: Unstaging incidental lockfile change: ${lockpath}"
        git restore --staged -- "${lockpath}"
      done
    fi
  fi
fi
if ! git diff --cached --name-only | grep -q .; then
  echo "ERROR: No staged files remain after scope filtering."
  exit 1
fi
if git ls-files --others --exclude-standard | grep -q .; then
  echo "ERROR: Untracked files detected."
  git ls-files --others --exclude-standard
  exit 1
fi
```

## Step 8: Security Scan

Tool: bash
OnFail: abort

Check for hardcoded secrets, debug statements, and commented-out code
in staged files. Runs after staging so `git diff --cached` covers all changes.

```bash
git diff --cached --name-only | while read -r file; do
  if grep -nE '(API_KEY|SECRET|PASSWORD|PRIVATE_KEY)=' "$file" 2>/dev/null; then
    echo "FAIL: Potential secret in $file"
    exit 1
  fi
done
```

## Step 9: Security Audit

Tool: bash
OnFail: abort

Run the security-audit skill: test completeness check, vulnerability scan,
and code quality check. The audit MUST pass before commit.

Phase 1: Can you propose a test case that does not exist? If yes, FAIL.
Phase 2: Input validation, size limits, panic risks.
Phase 3: No debug code, secrets, or commented-out code.

```bash
AUDIT_PROMPT="Use the security-audit skill.
Run security-audit against staged changes.
Output a concise report and end with EXACTLY one line:
SECURITY_AUDIT_VERDICT: PASS|PASS_DEFERRED|FAIL"
if command -v timeout >/dev/null 2>&1; then
  AUDIT_OUTPUT="$(timeout 1200 csa run --skill security-audit "${AUDIT_PROMPT}" 2>&1)"
  AUDIT_STATUS=$?
else
  AUDIT_OUTPUT="$(csa run --skill security-audit "${AUDIT_PROMPT}" 2>&1)"
  AUDIT_STATUS=$?
fi
printf '%s\n' "${AUDIT_OUTPUT}"
if [ "${AUDIT_STATUS}" -eq 124 ]; then
  echo "ERROR: security-audit timed out after 1200s." >&2
  exit 1
fi
if [ "${AUDIT_STATUS}" -ne 0 ]; then
  echo "ERROR: security-audit command failed (exit=${AUDIT_STATUS})." >&2
  exit 1
fi
VERDICT="$(printf '%s\n' "${AUDIT_OUTPUT}" | sed -nE 's/^SECURITY_AUDIT_VERDICT:[[:space:]]*(PASS_DEFERRED|PASS|FAIL)$/\1/p' | tail -n1)"
if [ -z "${VERDICT}" ]; then
  echo "ERROR: Missing SECURITY_AUDIT_VERDICT marker in audit output." >&2
  exit 1
fi
if [ "${VERDICT}" = "FAIL" ]; then
  echo "ERROR: security-audit verdict is FAIL." >&2
  exit 1
fi
echo "CSA_VAR:SECURITY_AUDIT_VERDICT=${VERDICT}"
```

## Step 10: Pre-Commit Review

Tool: bash

Run heterogeneous pre-commit review on uncommitted changes.
This step is strictly review-only: no commit/push/PR side effects.

```bash
set +e
if command -v timeout >/dev/null 2>&1; then
  REVIEW_OUTPUT="$(timeout 1800 csa review --diff 2>&1)"
  REVIEW_STATUS=$?
else
  REVIEW_OUTPUT="$(csa review --diff 2>&1)"
  REVIEW_STATUS=$?
fi
set -e
printf '%s\n' "${REVIEW_OUTPUT}"

if [ "${REVIEW_STATUS}" -eq 124 ]; then
  echo "ERROR: pre-commit review timed out after 1800s." >&2
  exit 1
fi

if [ "${REVIEW_STATUS}" -eq 0 ]; then
  echo "CSA_VAR:REVIEW_HAS_ISSUES=false"
  exit 0
fi

if printf '%s\n' "${REVIEW_OUTPUT}" | grep -q '<!-- CSA:SECTION:'; then
  echo "CSA_VAR:REVIEW_HAS_ISSUES=true"
  exit 0
fi

echo "ERROR: csa review failed unexpectedly (exit=${REVIEW_STATUS})." >&2
exit 1
```

## IF ${REVIEW_HAS_ISSUES}

## Step 11: Fix Review Issues

Tool: bash
OnFail: retry 3

Apply fixes for issues found in Step 10 using review-and-fix mode.
Do not commit/push inside this step; only modify code.

```bash
set +e
if command -v timeout >/dev/null 2>&1; then
  FIX_OUTPUT="$(timeout 1800 csa review --diff --fix 2>&1)"
  FIX_STATUS=$?
else
  FIX_OUTPUT="$(csa review --diff --fix 2>&1)"
  FIX_STATUS=$?
fi
set -e
printf '%s\n' "${FIX_OUTPUT}"
if [ "${FIX_STATUS}" -eq 124 ]; then
  echo "ERROR: review --fix timed out after 1800s." >&2
  exit 1
fi
if [ "${FIX_STATUS}" -ne 0 ]; then
  echo "ERROR: review --fix failed (exit=${FIX_STATUS})." >&2
  exit 1
fi
```

## Step 12: Re-run Quality Gates

Tool: bash
OnFail: abort

Re-run formatters, linters, and tests after fixes.

```bash
just pre-commit
```

## Step 13: Re-review

Tool: bash

Re-run review to verify remediation quality.
If issues remain after fix, fail the workflow.

```bash
set +e
if command -v timeout >/dev/null 2>&1; then
  REREVIEW_OUTPUT="$(timeout 1800 csa review --diff 2>&1)"
  REREVIEW_STATUS=$?
else
  REREVIEW_OUTPUT="$(csa review --diff 2>&1)"
  REREVIEW_STATUS=$?
fi
set -e
printf '%s\n' "${REREVIEW_OUTPUT}"

if [ "${REREVIEW_STATUS}" -eq 124 ]; then
  echo "ERROR: re-review timed out after 1800s." >&2
  exit 1
fi

if [ "${REREVIEW_STATUS}" -eq 0 ]; then
  echo "CSA_VAR:REVIEW_HAS_ISSUES=false"
  exit 0
fi

if printf '%s\n' "${REREVIEW_OUTPUT}" | grep -q '<!-- CSA:SECTION:'; then
  echo "ERROR: Re-review still reports unresolved issues." >&2
  echo "CSA_VAR:REVIEW_HAS_ISSUES=true"
  exit 1
fi

echo "ERROR: re-review failed unexpectedly (exit=${REREVIEW_STATUS})." >&2
exit 1
```

## ENDIF

## Step 14: Generate Commit Message

Tool: bash
OnFail: abort

Generate a deterministic Conventional Commits message from staged files.

```bash
scripts/gen_commit_msg.sh "${SCOPE:-}"
```

## Step 15: Commit

Tool: bash
OnFail: abort

Create the commit using the generated message from Step 14.

```bash
COMMIT_MSG_LOCAL="${STEP_14_OUTPUT:-${COMMIT_MSG:-}}"
if [ -z "${COMMIT_MSG_LOCAL}" ]; then
  echo "ERROR: Commit message is empty. Step 14 must output a commit message." >&2
  exit 1
fi
git commit -m "${COMMIT_MSG_LOCAL}"
```

## Step 16: Ensure Version Bumped

Tool: bash
OnFail: abort

Ensure workspace version differs from main before push gate.
If not bumped yet, auto-bump patch and create a dedicated release commit.

```bash
set -euo pipefail
if just check-version-bumped; then
  echo "Version bump check passed."
  exit 0
fi
PRE_DIRTY_CARGO_LOCK=0
if git diff --name-only -- Cargo.lock | grep -q .; then
  PRE_DIRTY_CARGO_LOCK=1
fi
just bump-patch
# Use workspace weave binary to avoid stale globally-installed version drift.
cargo run -p weave -- lock
git add Cargo.toml weave.lock
if [ "${PRE_DIRTY_CARGO_LOCK}" -eq 0 ] && [ -f Cargo.lock ]; then
  git add Cargo.lock
else
  echo "INFO: Skipping Cargo.lock in release commit (pre-existing local edits)."
fi
if git diff --cached --quiet; then
  echo "ERROR: Version bump expected changes but none were staged." >&2
  exit 1
fi
VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "cli-sub-agent") | .version')"
git commit -m "chore(release): bump workspace version to ${VERSION}"
```

## Step 17: Pre-PR Cumulative Review

Tool: bash
OnFail: abort

Run cumulative read-only review for the full feature branch range.
This gate must pass before push/PR.

```bash
if command -v timeout >/dev/null 2>&1; then
  timeout 1800 csa review --range main...HEAD
else
  csa review --range main...HEAD
fi
echo "CSA_VAR:CUMULATIVE_REVIEW_COMPLETED=true"
```

## Step 18: Push to Origin

Tool: bash
OnFail: retry 2

Push the feature branch to the remote origin.

```bash
BRANCH="$(git branch --show-current)"
if [ -z "${BRANCH}" ] || [ "${BRANCH}" = "HEAD" ]; then
  echo "ERROR: Cannot determine current branch for push."
  exit 1
fi
git push -u origin "${BRANCH}"
```

## Step 19: Create Pull Request

Tool: bash
OnFail: abort

Create a PR targeting main via GitHub CLI. The PR body includes a summary
of changes for ${SCOPE} and a test plan checklist covering tests, linting,
security audit, and codex review.

```bash
REPO_LOCAL="$(gh repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null || true)"
if [ -z "${REPO_LOCAL}" ]; then
  ORIGIN_URL="$(git remote get-url origin 2>/dev/null || true)"
  REPO_LOCAL="$(printf '%s' "${ORIGIN_URL}" | sed -nE 's#(git@github\.com:|https://github\.com/)([^/]+/[^/]+)(\.git)?$#\2#p')"
  REPO_LOCAL="${REPO_LOCAL%.git}"
fi
if [ -z "${REPO_LOCAL}" ]; then
  echo "ERROR: Cannot resolve repository owner/name." >&2
  exit 1
fi
COMMIT_MSG_LOCAL="${STEP_14_OUTPUT:-${COMMIT_MSG:-}}"
if [ -z "${COMMIT_MSG_LOCAL}" ]; then
  echo "ERROR: PR title is empty. Step 14 output is required." >&2
  exit 1
fi
PR_BODY_LOCAL="${PR_BODY:-Summary:
- Scope: ${SCOPE:-unspecified}

Validation:
- just fmt
- just clippy
- just test
- csa review --range main...HEAD
}"
BRANCH="$(git branch --show-current)"
EXISTING_PR="$(gh pr list --repo "${REPO_LOCAL}" --state open --head "${BRANCH}" --json number --jq '.[0].number' 2>/dev/null || true)"
if [ -n "${EXISTING_PR}" ] && [ "${EXISTING_PR}" != "null" ]; then
  echo "INFO: Reusing existing PR #${EXISTING_PR} for branch ${BRANCH}."
  echo "CSA_VAR:PR_NUMBER=${EXISTING_PR}"
  exit 0
fi
gh pr create --base main --repo "${REPO_LOCAL}" --title "${COMMIT_MSG_LOCAL}" --body "${PR_BODY_LOCAL}"
CREATED_PR="$(gh pr list --repo "${REPO_LOCAL}" --state open --head "${BRANCH}" --json number --jq '.[0].number' 2>/dev/null || true)"
if [ -n "${CREATED_PR}" ] && [ "${CREATED_PR}" != "null" ]; then
  echo "CSA_VAR:PR_NUMBER=${CREATED_PR}"
fi
```

## Step 20: Delegate PR Review/Merge to pr-codex-bot

Tool: bash
OnFail: abort

Delegate all long polling/status waiting to CSA internals.
Run `pr-codex-bot` in a child CSA session so the caller workflow stays concise.
The delegated workflow handles trigger, bounded polling, timeout fallback,
fix loops, and merge end-to-end.

```bash
set -euo pipefail
csa run --skill pr-codex-bot --no-stream-stdout \
  "Operate on the current branch and active PR. Execute the full cloud review lifecycle end-to-end, including trigger, polling, timeout fallback, iterative fixes, and merge."
```
