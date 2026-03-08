---
name = "review-loop"
description = "Bounded iterative review-fix loop for quality convergence"
allowed-tools = "Bash, Read, Grep, Edit, Task"
tier = "tier-2-standard"
version = "0.1.0"
---

# Review Loop

Bounded iterative review-fix loop for quality convergence.

## Step 1: Variables

Tool: bash
OnFail: abort

Initialize and declare workflow variables.

- `${REVIEW_HAS_ISSUES}`: `"true"` if review found issues
- `${ROUND}`: Current round number (starts at 1)
- `${MAX_ROUNDS}`: Maximum review-fix rounds (default: 2)
- `${REMAINING_ISSUES}`: Summary of unfixed issues (if loop exhausted)

```bash
# Force weave to pick up these variables
: "${REVIEW_HAS_ISSUES}" "${ROUND}" "${MAX_ROUNDS}" "${REMAINING_ISSUES}"
echo "Variables initialized."
```

## Step 2: Review Changes

Tool: bash
OnFail: skip

Run heterogeneous code review on current diff.

```bash
csa review --diff
```

Parse the output to determine if issues were found.
Set `${REVIEW_HAS_ISSUES}` to `"true"` or `"false"`.

## Step 3: Evaluate Review Result

Tool: bash
OnFail: abort

If review found no issues (`${REVIEW_HAS_ISSUES}` == `"false"`), report success and stop.
If issues were found, proceed to fix step.

## Step 4: Fix Issues

Tool: bash
OnFail: skip

Apply fixes for all issues reported in the review.
Focus on Critical and High severity first.
After fixing, stage changes for re-review.

## IF ${REVIEW_HAS_ISSUES}

## Step 5: Round Check

Tool: bash
OnFail: abort

Increment `${ROUND}` counter.
If `${ROUND}` >= `${MAX_ROUNDS}` (default 2), set `${REMAINING_ISSUES}` with a summary
of any unfixed issues and exit.
Otherwise, loop back to Step 2 for re-review.

## ENDIF
