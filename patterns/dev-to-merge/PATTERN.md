---
name = "dev-to-merge"
description = "Full development cycle from branch creation through commit, PR, codex-bot review, and merge"
allowed-tools = "Bash, Read, Edit, Write, Grep, Glob, Task"
tier = "tier-3-complex"
version = "0.1.0"
---

# Dev-to-Merge Workflow

End-to-end development workflow: implement code on a feature branch, pass all
quality gates, commit with Conventional Commits, create a PR, run codex-bot
review loop, and merge to main.

## Step 1: Validate Branch

Tool: bash
OnFail: abort

Verify the current branch is a feature branch, not a protected branch.
If on main or dev, abort immediately.

```bash
BRANCH="${BRANCH}"
DEFAULT_BRANCH=$(git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's@^refs/remotes/origin/@@')
if [ -z "$DEFAULT_BRANCH" ]; then DEFAULT_BRANCH="main"; fi
if [ "$BRANCH" = "$DEFAULT_BRANCH" ] || [ "$BRANCH" = "dev" ]; then
  echo "ERROR: Cannot work directly on $BRANCH. Create a feature branch."
  exit 1
fi
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
  elif ! printf '%s\n' "${STAGED_FILES}" | grep -Ev '(^|/)(Cargo\.lock|weave\.lock|package-lock\.json|pnpm-lock\.yaml|yarn\.lock|go\.sum)$' | grep -q .; then
    echo "INFO: Lockfile-only staged change detected; preserving staged lockfiles."
  else
    MATCHED_LOCKFILES="$(printf '%s\n' "${STAGED_FILES}" | awk '$0 ~ /(^|\/)(Cargo\.lock|weave\.lock|package-lock\.json|pnpm-lock\.yaml|yarn\.lock|go\.sum)$/ { print }')"
    if [ -n "${MATCHED_LOCKFILES}" ]; then
      printf '%s\n' "${MATCHED_LOCKFILES}" | while read -r lockpath; do
        echo "INFO: Unstaging incidental lockfile change: ${lockpath}"
        git restore --staged -- "${lockpath}"
        if [ "${CSA_KEEP_FILTERED_LOCKFILES_IN_WORKTREE:-0}" != "1" ]; then
          echo "INFO: Cleaning incidental lockfile from worktree: ${lockpath}"
          git restore --worktree -- "${lockpath}"
        fi
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

Tool: csa
Tier: tier-1-quick

Generate a Conventional Commits message from the staged diff.
The message must follow the format: `type(${SCOPE}): description`.

```bash
csa run "Run 'git diff --staged' and generate a Conventional Commits message. Scope: ${SCOPE}"
```

## Step 13: Commit

Tool: bash
OnFail: abort

Create the commit using the generated message: ${COMMIT_MSG}.

```bash
git commit -m "${COMMIT_MSG}"
```

## Step 13a: Ensure Version Bumped

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
PRE_DIRTY_LOCKS="$(git diff --name-only -- Cargo.lock weave.lock)"
just bump-patch
weave lock
git add Cargo.toml
for lockfile in Cargo.lock weave.lock; do
  if printf '%s\n' "${PRE_DIRTY_LOCKS}" | grep -qx "${lockfile}"; then
    echo "INFO: Skipping ${lockfile} in release commit (pre-existing local edits)."
    continue
  fi
  if [ -f "${lockfile}" ]; then
    git add "${lockfile}"
  fi
done
if git diff --cached --quiet; then
  echo "ERROR: Version bump expected changes but none were staged." >&2
  exit 1
fi
VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "cli-sub-agent") | .version')"
git commit -m "chore(release): bump workspace version to ${VERSION}"
```

## Step 13b: Pre-PR Cumulative Review

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

## Step 14: Push to Origin

Tool: bash
OnFail: retry 2

Push the feature branch to the remote origin.

```bash
git push -u origin "${BRANCH}"
```

## Step 15: Create Pull Request

Tool: bash
OnFail: abort

Create a PR targeting main via GitHub CLI. The PR body includes a summary
of changes for ${SCOPE} and a test plan checklist covering tests, linting,
security audit, and codex review.

```bash
gh pr create --base main --title "${COMMIT_MSG}" --body "${PR_BODY}"
```

## Step 16: Trigger Codex Bot Review

Tool: bash

Trigger the cloud codex review bot on the newly created PR.
Capture the PR number for polling.

```bash
set -euo pipefail
PR_NUM=$(gh pr view --json number -q '.number')
gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review"
SELF_LOGIN=$(gh api user -q '.login')
COMMENTS_PAYLOAD=$(gh pr view "${PR_NUM}" --repo "${REPO}" --json comments)
TRIGGER_TS=$(printf '%s' "${COMMENTS_PAYLOAD}" | jq -r --arg me "${SELF_LOGIN}" '[.comments[]? | select(.author.login == $me and .body == "@codex review")] | sort_by(.createdAt) | last | .createdAt // empty')
if [ -z "${TRIGGER_TS}" ]; then
  TRIGGER_TS=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
fi
printf 'PR_NUM=%s\nTRIGGER_TS=%s\n' "${PR_NUM}" "${TRIGGER_TS}"
```

## Step 17: Poll for Bot Response

Tool: bash
OnFail: skip

Poll for bot review response with a bounded timeout (max 10 minutes).
If the bot does not respond, fall through to UNAVAILABLE handling.

```bash
TIMEOUT=600; INTERVAL=30; ELAPSED=0
PR_NUM_FROM_STEP="$(printf '%s\n' "${STEP_16_OUTPUT:-}" | sed -n 's/^PR_NUM=//p' | tail -n1)"
TRIGGER_TS="$(printf '%s\n' "${STEP_16_OUTPUT:-}" | sed -n 's/^TRIGGER_TS=//p' | tail -n1)"
if [ -z "${PR_NUM_FROM_STEP}" ]; then PR_NUM_FROM_STEP="${PR_NUM}"; fi
if [ -z "${TRIGGER_TS}" ]; then TRIGGER_TS="1970-01-01T00:00:00Z"; fi
while [ "$ELAPSED" -lt "$TIMEOUT" ]; do
  PAYLOAD=$(gh pr view "${PR_NUM_FROM_STEP}" --repo "${REPO}" --json comments,reviews)
  BOT_COMMENTS=$(printf '%s' "${PAYLOAD}" | jq -r --arg ts "${TRIGGER_TS}" '[.comments[]? | select(.createdAt >= $ts and (.author.login | ascii_downcase | test("codex|bot|connector")) and (((.body // "") | ascii_downcase | contains("@codex review")) | not))] | length')
  BOT_REVIEWS=$(printf '%s' "${PAYLOAD}" | jq -r --arg ts "${TRIGGER_TS}" '[.reviews[]? | select(.submittedAt >= $ts and (.author.login | ascii_downcase | test("codex|bot|connector")))] | length')
  if [ "${BOT_COMMENTS}" -gt 0 ] || [ "${BOT_REVIEWS}" -gt 0 ]; then
    echo "Bot response received."
    exit 0
  fi
  sleep "$INTERVAL"
  ELAPSED=$((ELAPSED + INTERVAL))
done
echo "Bot did not respond within timeout."
exit 1
```

## IF ${BOT_HAS_ISSUES}

## Step 18: Evaluate Bot Comments

Tool: csa
Tier: tier-2-standard

For each bot comment, classify as:
- Category A (already fixed): react and acknowledge
- Category B (suspected false positive): queue for arbitration
- Category C (real issue): react and queue for fix

## FOR comment IN ${BOT_COMMENTS}

## Step 19: Process Comment

Tool: csa

Evaluate this specific bot comment against the current code state.
Determine category (A/B/C) and take appropriate action.

## IF ${COMMENT_IS_FALSE_POSITIVE}

## Step 20: Arbitrate False Positive

Tool: csa
Tier: tier-2-standard

Run `csa debate` to get an independent second opinion on the suspected
false positive. The arbiter MUST be a different model family.

```bash
csa debate "A code reviewer flagged: ${COMMENT_TEXT}. Evaluate independently."
```

## ELSE

## Step 21: Fix Real Issue

Tool: csa
Tier: tier-2-standard
OnFail: retry 2

Fix the real issue identified by the bot. Commit the fix.

## ENDIF

## ENDFOR

## Step 22: Push Fixes and Re-trigger Review

Tool: bash

Push all fix commits and trigger a new round of codex review.

```bash
set -euo pipefail
git push origin "${BRANCH}"
gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review"
SELF_LOGIN=$(gh api user -q '.login')
COMMENTS_PAYLOAD=$(gh pr view "${PR_NUM}" --repo "${REPO}" --json comments)
TRIGGER_TS=$(printf '%s' "${COMMENTS_PAYLOAD}" | jq -r --arg me "${SELF_LOGIN}" '[.comments[]? | select(.author.login == $me and .body == "@codex review")] | sort_by(.createdAt) | last | .createdAt // empty')
if [ -z "${TRIGGER_TS}" ]; then
  TRIGGER_TS=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
fi
printf 'PR_NUM=%s\nTRIGGER_TS=%s\n' "${PR_NUM}" "${TRIGGER_TS}"
```

## ELSE

## Step 23: Bot Review Clean

No issues found by the codex bot. Proceed to merge.

## ENDIF

## Step 24: Merge PR

Tool: bash
OnFail: abort

Squash-merge the PR and update local main.

```bash
gh pr merge "${PR_NUM}" --repo "${REPO}" --squash --delete-branch
git checkout main && git pull origin main
```
