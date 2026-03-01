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

## Step 1.5: Plan with mktd (Debate Required)

Tool: bash
OnFail: abort

Generate or refresh a branch TODO plan through `mktd` before development gates.
This step MUST pass through mktd's built-in debate phase and save a TODO.

## INCLUDE mktd

```bash
set -euo pipefail
CURRENT_BRANCH="$(git branch --show-current)"
FEATURE_INPUT="${SCOPE:-current branch changes pending merge}"
MKTD_PROMPT="Plan dev-to-merge execution for branch ${CURRENT_BRANCH}. Scope: ${FEATURE_INPUT}. Must execute full mktd workflow and save TODO."
MKTD_OUTPUT="$(csa run --skill mktd "${MKTD_PROMPT}" 2>&1)"
MKTD_STATUS=$?
printf '%s\n' "${MKTD_OUTPUT}"
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
grep -qE '^- \[ \] .+' "${TODO_PATH}" || { echo "ERROR: TODO missing checkbox tasks: ${TODO_PATH}" >&2; exit 1; }
grep -q 'DONE WHEN:' "${TODO_PATH}" || { echo "ERROR: TODO missing DONE WHEN clauses: ${TODO_PATH}" >&2; exit 1; }
printf 'MKTD_TODO_TIMESTAMP=%s\nMKTD_TODO_PATH=%s\n' "${LATEST_TS}" "${TODO_PATH}"
```

## Step 2: Run Formatters

Tool: bash
OnFail: retry 2

Run the project formatter to ensure consistent code style.

```bash
just fmt
```

## Step 3: Run Linters

Tool: bash
OnFail: retry 2

Run linters to catch static analysis issues.

```bash
just clippy
```

## Step 4: Run Tests

Tool: bash
OnFail: abort

Run the full test suite. All tests must pass before proceeding.

```bash
just test
```

## Step 5: Stage Changes

Tool: bash

Stage all modified and new files relevant to ${SCOPE}.
Verify no untracked files remain.

```bash
git add -A
if ! printf '%s' "${SCOPE:-}" | grep -Eqi 'release|version|lock|deps|dependency'; then
  STAGED_FILES="$(git diff --cached --name-only)"
  if printf '%s\n' "${STAGED_FILES}" | grep -Eq '(^|/)Cargo\.toml$|(^|/)package\.json$|(^|/)pnpm-workspace\.yaml$|(^|/)go\.mod$'; then
    echo "INFO: Dependency manifest change detected; preserving staged lockfiles."
  elif ! printf '%s\n' "${STAGED_FILES}" | grep -Ev '(^|/)(Cargo\.lock|package-lock\.json|pnpm-lock\.yaml|yarn\.lock|go\.sum)$' | grep -q .; then
    echo "INFO: Lockfile-only staged change detected; preserving staged lockfiles."
  else
    MATCHED_LOCKFILES="$(printf '%s\n' "${STAGED_FILES}" | awk '$0 ~ /(^|\/)(Cargo\.lock|package-lock\.json|pnpm-lock\.yaml|yarn\.lock|go\.sum)$/ { print }')"
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

## Step 6: Security Scan

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

## Step 7: Security Audit

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
echo "SECURITY_AUDIT_VERDICT=${VERDICT}"
```

## Step 8: Pre-Commit Review

Tool: csa
Tier: tier-2-standard

Run heterogeneous code review on all uncommitted changes versus HEAD.
The reviewer MUST be a different model family than the code author.

```bash
csa review --diff
```

Review output includes AGENTS.md compliance checklist.

## IF ${REVIEW_HAS_ISSUES}

## Step 9: Fix Review Issues

Tool: csa
Tier: tier-2-standard
OnFail: retry 3

Fix each issue identified by the pre-commit review.
Preserve original code intent. Do NOT delete code to silence warnings.

## Step 10: Re-run Quality Gates

Tool: bash
OnFail: abort

Re-run formatters, linters, and tests after fixes.

```bash
just pre-commit
```

## Step 11: Re-review

Tool: csa
Tier: tier-2-standard

Run `csa review --diff` again to verify all issues are resolved.
Loop back to Step 9 if issues persist (max 3 rounds).

## ENDIF

## Step 12: Generate Commit Message

Tool: bash
OnFail: abort

Generate a deterministic Conventional Commits message from staged files.

```bash
scripts/gen_commit_msg.sh "${SCOPE:-}"
```

## Step 13: Commit

Tool: bash
OnFail: abort

Create the commit using the generated message from Step 12.

```bash
COMMIT_MSG_LOCAL="${STEP_12_OUTPUT:-${COMMIT_MSG:-}}"
if [ -z "${COMMIT_MSG_LOCAL}" ]; then
  echo "ERROR: Commit message is empty. Step 12 must output a commit message." >&2
  exit 1
fi
git commit -m "${COMMIT_MSG_LOCAL}"
```

## Step 14: Ensure Version Bumped

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

## Step 15: Pre-PR Cumulative Review

Tool: csa
Tier: tier-2-standard
OnFail: abort

Run a cumulative review covering ALL commits on the feature branch since main.
This is distinct from Step 8's per-commit review (`csa review --diff`):
- Step 8 reviews uncommitted changes (staged diff) — single-commit granularity.
- This step reviews the full feature branch — catches cross-commit issues.

MANDATORY: This review MUST pass before pushing to origin.

```bash
csa review --range main...HEAD
CUMULATIVE_REVIEW_COMPLETED=true
```

## Step 16: Push to Origin

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

## Step 17: Create Pull Request

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
COMMIT_MSG_LOCAL="${STEP_12_OUTPUT:-${COMMIT_MSG:-}}"
if [ -z "${COMMIT_MSG_LOCAL}" ]; then
  echo "ERROR: PR title is empty. Step 12 output is required." >&2
  exit 1
fi
PR_BODY_LOCAL="${PR_BODY:-## Summary
- Scope: ${SCOPE:-unspecified}

## Validation
- just fmt
- just clippy
- just test
- csa review --range main...HEAD
}"
gh pr create --base main --repo "${REPO_LOCAL}" --title "${COMMIT_MSG_LOCAL}" --body "${PR_BODY_LOCAL}"
```

## Step 18: Trigger Codex Bot Review

Tool: bash

Trigger the cloud codex review bot on the newly created PR.
Capture the PR number for polling.

```bash
set -euo pipefail
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
PR_NUM=$(gh pr view --json number -q '.number')
gh pr comment "${PR_NUM}" --repo "${REPO_LOCAL}" --body "@codex review"
SELF_LOGIN=$(gh api user -q '.login')
COMMENTS_PAYLOAD=$(gh pr view "${PR_NUM}" --repo "${REPO_LOCAL}" --json comments)
TRIGGER_TS=$(printf '%s' "${COMMENTS_PAYLOAD}" | jq -r --arg me "${SELF_LOGIN}" '[.comments[]? | select(.author.login == $me and .body == "@codex review")] | sort_by(.createdAt) | last | .createdAt // empty')
if [ -z "${TRIGGER_TS}" ]; then
  TRIGGER_TS=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
fi
printf 'PR_NUM=%s\nTRIGGER_TS=%s\n' "${PR_NUM}" "${TRIGGER_TS}"
```

## Step 19: Poll for Bot Response

Tool: bash
OnFail: abort

Poll for bot review response with a bounded timeout (max 10 minutes).
Output `1` when bot findings are present; output empty string otherwise.

```bash
TIMEOUT=600; INTERVAL=30; ELAPSED=0
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
PR_NUM_FROM_STEP="$(printf '%s\n' "${STEP_18_OUTPUT:-}" | sed -n 's/^PR_NUM=//p' | tail -n1)"
TRIGGER_TS="$(printf '%s\n' "${STEP_18_OUTPUT:-}" | sed -n 's/^TRIGGER_TS=//p' | tail -n1)"
if [ -z "${PR_NUM_FROM_STEP}" ]; then PR_NUM_FROM_STEP="${PR_NUM}"; fi
if [ -z "${TRIGGER_TS}" ]; then TRIGGER_TS="1970-01-01T00:00:00Z"; fi
while [ "$ELAPSED" -lt "$TIMEOUT" ]; do
  BOT_INLINE_COMMENTS=$(gh api "repos/${REPO_LOCAL}/pulls/${PR_NUM_FROM_STEP}/comments?per_page=100" | jq -r --arg ts "${TRIGGER_TS}" '[.[]? | select(.created_at >= $ts and (.user.login | ascii_downcase | test("codex|bot|connector")))] | length')
  BOT_PR_COMMENTS=$(gh api "repos/${REPO_LOCAL}/issues/${PR_NUM_FROM_STEP}/comments?per_page=100" | jq -r --arg ts "${TRIGGER_TS}" '[.[]? | select((.created_at // "") >= $ts and (.user.login | ascii_downcase | test("codex|bot|connector")) and (((.body // "") | ascii_downcase | contains("@codex review")) | not))] | length')
  BOT_REVIEWS=$(gh api "repos/${REPO_LOCAL}/pulls/${PR_NUM_FROM_STEP}/reviews?per_page=100" | jq -r --arg ts "${TRIGGER_TS}" '[.[]? | select((.submitted_at // "") >= $ts and (.user.login | ascii_downcase | test("codex|bot|connector")))] | length')
  if [ "${BOT_INLINE_COMMENTS}" -gt 0 ] || [ "${BOT_PR_COMMENTS}" -gt 0 ] || [ "${BOT_REVIEWS}" -gt 0 ]; then
    echo "1"
    exit 0
  fi
  sleep "$INTERVAL"
  ELAPSED=$((ELAPSED + INTERVAL))
done
echo ""
```

## IF ${STEP_19_OUTPUT}

## Step 20: Evaluate Bot Comments

Tool: csa
Tier: tier-2-standard

Evaluate all inline bot findings on the PR and produce a consolidated action plan.
List suspected false positives and real defects separately.

## Step 21: Arbitrate Disputed Findings

Tool: csa

For disputed findings, run independent arbitration using `csa debate` and
produce a verdict for each disputed item.

## Step 22: Fix Confirmed Issues

Tool: csa
Tier: tier-2-standard

Implement fixes for confirmed bot findings and create commit(s) with clear
messages. Do not modify unrelated files.

## Step 23: Re-run Local Review After Fixes

Tool: csa
Tier: tier-2-standard
OnFail: retry 2

Run `csa review --diff` to validate fixes before re-triggering cloud review.

## Step 24: Push Fixes and Re-trigger Review

Tool: bash

Push all fix commits and trigger a new round of codex review.

```bash
set -euo pipefail
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
BRANCH="$(git branch --show-current)"
if [ -z "${BRANCH}" ] || [ "${BRANCH}" = "HEAD" ]; then
  echo "ERROR: Cannot determine current branch for push."
  exit 1
fi
git push origin "${BRANCH}"
gh pr comment "${PR_NUM}" --repo "${REPO_LOCAL}" --body "@codex review"
SELF_LOGIN=$(gh api user -q '.login')
COMMENTS_PAYLOAD=$(gh pr view "${PR_NUM}" --repo "${REPO_LOCAL}" --json comments)
TRIGGER_TS=$(printf '%s' "${COMMENTS_PAYLOAD}" | jq -r --arg me "${SELF_LOGIN}" '[.comments[]? | select(.author.login == $me and .body == "@codex review")] | sort_by(.createdAt) | last | .createdAt // empty')
if [ -z "${TRIGGER_TS}" ]; then
  TRIGGER_TS=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
fi
printf 'PR_NUM=%s\nTRIGGER_TS=%s\n' "${PR_NUM}" "${TRIGGER_TS}"
```

## ELSE

## Step 25: Bot Review Clean

No issues found by the codex bot. Proceed to merge.

## ENDIF

## Step 26: Merge PR

Tool: bash
OnFail: abort

Squash-merge the PR and update local main.

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
gh pr merge "${PR_NUM}" --repo "${REPO_LOCAL}" --squash --delete-branch
git checkout main && git pull origin main
```
