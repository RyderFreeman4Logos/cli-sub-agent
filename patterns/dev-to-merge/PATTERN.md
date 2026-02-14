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

Tool: csa
Tier: tier-2-standard
OnFail: abort

Run the security-audit skill: test completeness check, vulnerability scan,
and code quality check. The audit MUST pass before commit.

Phase 1: Can you propose a test case that does not exist? If yes, FAIL.
Phase 2: Input validation, size limits, panic risks.
Phase 3: No debug code, secrets, or commented-out code.

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
NOTE: In production, this step should invoke the `/commit` skill which
enforces security audit, test completeness, and AGENTS.md compliance.
The raw `git commit` here demonstrates the skill-lang format only.

```bash
git commit -m "${COMMIT_MSG}"
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
NOTE: In production, Steps 15-24 should invoke the `/pr-codex-bot` skill
which handles the full review-trigger-procedure, bounded polling, false-positive
arbitration, and merge atomically. The manual flow here demonstrates skill-lang.

```bash
PR_NUM=$(gh pr view --json number -q '.number')
gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review"
```

## Step 17: Poll for Bot Response

Tool: bash
OnFail: skip

Poll for bot review response with a bounded timeout (max 10 minutes).
If the bot does not respond, fall through to UNAVAILABLE handling.

## IF ${BOT_HAS_ISSUES}

## Step 18: Evaluate Bot Comments

Tool: claude-code
Tier: tier-3-complex

For each bot comment, classify as:
- Category A (already fixed): react and acknowledge
- Category B (suspected false positive): queue for arbitration
- Category C (real issue): react and queue for fix

## FOR comment IN ${BOT_COMMENTS}

## Step 19: Process Comment

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
git push origin "${BRANCH}"
gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review"
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
