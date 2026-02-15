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

FORBIDDEN: self-dismissing bot comments, skipping debate for arbitration,
running Step 2 in background, creating PR without Step 2 completion.

## Step 1: Commit Changes

Tool: bash

Ensure all changes committed. Set WORKFLOW_BRANCH once (persists through
clean branch switches in Step 11).

```bash
WORKFLOW_BRANCH="$(git branch --show-current)"
```

## Step 2: Local Pre-PR Review (SYNCHRONOUS — MUST NOT background)

Tool: bash
OnFail: abort

Run cumulative local review covering all commits since main.
This is the FOUNDATION — without it, bot unavailability cannot safely merge.

```bash
csa review --branch main
```

## IF ${LOCAL_REVIEW_HAS_ISSUES}

## Step 3: Fix Local Review Issues

Tool: csa
Tier: tier-2-standard
OnFail: retry 3

Fix issues found by local review. Loop until clean (max 3 rounds).

## ENDIF

## Step 4: Push and Create PR

Tool: bash
OnFail: abort

```bash
git push -u origin "${WORKFLOW_BRANCH}"
gh pr create --base main --title "${PR_TITLE}" --body "${PR_BODY}"
PR_NUM=$(gh pr view --json number -q '.number')
```

## Step 5: Trigger Cloud Bot Review

Tool: bash

Trigger the cloud review bot. Capture PR number for polling.

```bash
gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review"
```

## Step 6: Poll for Bot Response

Tool: bash
OnFail: skip

Bounded poll (max 10 minutes). If bot unavailable, fall through —
local review (Step 2) already covers main...HEAD.

## IF ${BOT_UNAVAILABLE}

## Step 6a: Merge Without Bot

Tool: bash

Bot unavailable. Local review already guarantees coverage.
Proceed to merge directly.

```bash
gh pr merge "${PR_NUM}" --repo "${REPO}" --squash --delete-branch
git checkout main && git pull origin main
```

## ELSE

## IF ${BOT_HAS_ISSUES}

## Step 7: Evaluate Each Bot Comment

Tool: claude-code
Tier: tier-3-complex

## FOR comment IN ${BOT_COMMENTS}

Classify each comment:
- Category A (already fixed): react and acknowledge
- Category B (suspected false positive): queue for arbitration
- Category C (real issue): queue for fix

## IF ${COMMENT_IS_FALSE_POSITIVE}

## Step 8: Arbitrate via Debate

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

## Step 9: Fix Real Issue

Tool: csa
Tier: tier-2-standard
OnFail: retry 2

Fix the real issue. Commit the fix.

## ENDIF

## ENDFOR

## Step 10: Push Fixes and Re-trigger

Tool: bash

```bash
git push origin "${WORKFLOW_BRANCH}"
gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review"
```

Loop back to Step 6 (poll). Max 10 total iterations.

## ELSE

## Step 10a: Bot Review Clean

No issues found by bot. Proceed to merge.

## ENDIF

## ENDIF

## Step 11: Clean Resubmission (if needed)

Tool: bash

If fixes accumulated, create clean branch for final review.

```bash
CLEAN_BRANCH="${WORKFLOW_BRANCH}-clean"
git checkout -b "${CLEAN_BRANCH}"
git push -u origin "${CLEAN_BRANCH}"
gh pr create --base main --title "${PR_TITLE}" --body "${PR_BODY}"
```

## Step 12: Final Merge

Tool: bash
OnFail: abort

Squash-merge and update local main.

```bash
gh pr merge "${PR_NUM}" --repo "${REPO}" --squash --delete-branch
git checkout main && git pull origin main
```
