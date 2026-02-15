---
name = "code-review"
description = "GitHub PR code review via gh CLI with scale-adaptive strategy and AGENTS.md compliance"
allowed-tools = "Bash, Read, Grep, Glob"
tier = "tier-2-standard"
version = "0.1.0"
---

# Code Review

AI-powered GitHub PR code review with scale-adaptive strategy.
Small PRs reviewed directly, large PRs delegated to CSA.

## Step 1: Fetch PR Context

Tool: bash
OnFail: abort

Get PR metadata and diff statistics to assess scale.

```bash
gh pr view ${PR_NUM} --json title,body,author,files,additions,deletions,reviewDecision
gh pr diff ${PR_NUM} --stat
```

## Step 2: Assess PR Scale

Determine review strategy based on lines changed:
- Small (< 200 lines): direct review in main agent
- Medium (200-800 lines): direct review with progress tracking
- Large (> 800 lines): delegate to csa review

## Step 3: Authorship Check

Check git log for commits in scope. If Co-Authored-By matches
caller model family → use csa debate for review (self-authored code).
If commits by different tool/human → review directly.

## IF ${PR_IS_LARGE}

## Step 4a: Delegate Large PR to CSA

Tool: bash

Checkout PR branch locally, then delegate to CSA review.
Do NOT pre-read diff into main agent context.

```bash
gh pr checkout ${PR_NUM}
csa review --branch $(gh pr view --json baseRefName -q .baseRefName)
```

## ELSE

## Step 4b: Fetch Full Diff

Tool: bash

Read full diff for direct review.

```bash
gh pr diff ${PR_NUM}
```

## Step 5: Analyze Changes

Review each file for:
- Code quality (naming, organization, DRY)
- Security (input validation, SQL injection, XSS, secrets)
- Performance (N+1 queries, unnecessary allocations, blocking in async)
- Language-specific (ownership, lifetimes, unsafe for Rust)

## Step 6: AGENTS.md Compliance Check

For each changed file, discover AGENTS.md chain (root-to-leaf).
Check every applicable rule. Violations are at least P2, MUST/CRITICAL → P1.
Produce checklist with every rule checked.

## ENDIF

## Step 7: Generate Review

Produce structured review with:
- Summary (overall assessment)
- Critical Issues (must-fix before merge)
- Suggestions (recommended improvements)
- Nitpicks (optional style improvements)
- Questions (clarifications needed)
- AGENTS.md checklist (all rules checked)

## IF ${USER_REQUESTS_POSTING}

## Step 8: Submit Review to GitHub

Tool: bash
OnFail: abort

Post review comment to PR. Only when user explicitly requests.

```bash
gh pr comment ${PR_NUM} --body "${REVIEW_BODY}"
```

## ENDIF
