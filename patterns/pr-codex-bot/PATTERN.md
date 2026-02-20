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
debating stale comments without staleness check.

## Dispatcher Model Note

This pattern follows a 3-tier dispatcher architecture:
- **Tier 0 (Orchestrator)**: The main agent dispatches steps -- never touches code directly.
- **Tier 1 (Executors)**: CSA sub-agents and Task tool agents perform actual work.
- **Tier 2 (Sub-sub-agents)**: Spawned by Tier 1 for specific sub-tasks (invisible to Tier 0).

Each step below is annotated with its execution tier.

## Step 1: Commit Changes

> **Tier**: 0 (Orchestrator) -- lightweight shell command, no code reading.

Tool: bash

Ensure all changes committed. Set WORKFLOW_BRANCH once (persists through
clean branch switches in Step 11).

```bash
WORKFLOW_BRANCH="$(git branch --show-current)"
```

## Step 2: Local Pre-PR Review (SYNCHRONOUS — MUST NOT background)

> **Tier**: 1 (CSA executor) -- Tier 0 dispatches `csa review`, which spawns
> Tier 2 reviewer model(s) internally. Orchestrator waits for result.

Tool: bash
OnFail: abort

Run cumulative local review covering all commits since main.
This is the FOUNDATION — without it, bot unavailability cannot safely merge.

```bash
csa review --branch main
```

## IF ${LOCAL_REVIEW_HAS_ISSUES}

## Step 3: Fix Local Review Issues

> **Tier**: 1 (CSA executor) -- Tier 0 dispatches fix task to CSA. CSA reads
> code, applies fixes, and returns results. Orchestrator reviews outcome.

Tool: csa
Tier: tier-2-standard
OnFail: retry 3

Fix issues found by local review. Loop until clean (max 3 rounds).

## ENDIF

## Step 4: Push and Create PR

> **Tier**: 0 (Orchestrator) -- shell commands only, no code reading/writing.

Tool: bash
OnFail: abort

```bash
git push -u origin "${WORKFLOW_BRANCH}"
gh pr create --base main --title "${PR_TITLE}" --body "${PR_BODY}"
PR_NUM=$(gh pr view --json number -q '.number')
```

## Step 4a: Check Cloud Bot Configuration

> **Tier**: 0 (Orchestrator) -- config check, lightweight.

Tool: bash

Check whether cloud bot review is enabled for this project.

```bash
CLOUD_BOT=$(csa config get pr_review.cloud_bot --default true)
```

If `CLOUD_BOT` is `false`:
- Skip Steps 5 through 10 (cloud bot trigger, poll, classify, arbitrate, fix).
- Run supplementary local review: `csa review --range main..HEAD`.
- Jump directly to Step 12b (Final Merge — Direct).

## IF ${CLOUD_BOT} != "false"

## Step 5: Trigger Cloud Bot Review and Poll for Response

> **Tier**: 0 (Orchestrator) -- trigger + polling loop, no code analysis.
> This step is SELF-CONTAINED: trigger and poll are atomic. The orchestrator
> MUST NOT split these into separate manual actions.

Tool: bash
OnFail: skip

Trigger `@codex review` (skip if already commented on this HEAD), then poll
for bot response with bounded timeout (max 10 minutes, 30s interval).
If bot times out, set BOT_UNAVAILABLE and fall through — local review
(Step 2) already covers main...HEAD.

```bash
# --- Trigger @codex review (idempotent) ---
CURRENT_SHA="$(git rev-parse HEAD)"
EXISTING=$(gh api "repos/${REPO}/issues/${PR_NUM}/comments" \
  --jq "[.[] | select(.body | test(\"@codex review\")) | select(.body | test(\"${CURRENT_SHA}\") or (.updated_at > \"$(git log -1 --format=%cI HEAD~1 2>/dev/null || echo 1970-01-01)\"))] | length" 2>/dev/null || echo "0")
if [ "${EXISTING}" = "0" ]; then
  gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review"
fi

# --- Poll for bot response (max 10 min) ---
BOT_UNAVAILABLE=true
POLL_INTERVAL=30
MAX_WAIT=600
WAITED=0
while [ "${WAITED}" -lt "${MAX_WAIT}" ]; do
  sleep "${POLL_INTERVAL}"
  WAITED=$((WAITED + POLL_INTERVAL))
  BOT_REPLY=$(gh api "repos/${REPO}/issues/${PR_NUM}/comments" \
    --jq "[.[] | select(.user.type == \"Bot\" or .user.login == \"codex[bot]\" or .user.login == \"codex-bot\") | select(.created_at > \"$(git log -1 --format=%cI HEAD)\")] | length" 2>/dev/null || echo "0")
  if [ "${BOT_REPLY}" -gt 0 ] 2>/dev/null; then
    BOT_UNAVAILABLE=false
    break
  fi
  echo "Polling... ${WAITED}s / ${MAX_WAIT}s"
done

if [ "${BOT_UNAVAILABLE}" = "true" ]; then
  echo "Bot timed out after ${MAX_WAIT}s. Falling back to local review."
  # Fallback: run local csa review for coverage confirmation
  csa review --range main..HEAD 2>/dev/null || true
fi
```

## IF ${BOT_UNAVAILABLE}

## Step 6a: Merge Without Bot

> **Tier**: 0 (Orchestrator) -- merge command, no code analysis.

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

> **Tier**: 1 (claude-code / Task tool) -- Tier 0 dispatches comment
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

> **Tier**: 1 (CSA debate) -- Tier 0 dispatches to `csa debate`, which
> internally spawns Tier 2 independent models for adversarial evaluation.
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

> **Tier**: 1 (CSA executor) -- Tier 0 dispatches fix to CSA sub-agent.
> CSA reads code, applies fix, commits. Orchestrator verifies result.

Tool: csa
Tier: tier-2-standard
OnFail: retry 2

Fix the real issue (non-stale, non-false-positive). Commit the fix.

## ENDIF

## ENDFOR

## Step 10: Push Fixes and Re-trigger

> **Tier**: 0 (Orchestrator) -- shell commands to push and re-trigger bot.

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

## ENDIF
<!-- End of CLOUD_BOT != "false" block -->

## IF !(${BOT_UNAVAILABLE})

## IF ${FIXES_ACCUMULATED}

## Step 11: Clean Resubmission (if needed)

> **Tier**: 0 (Orchestrator) -- git branch management, no code reading.

Tool: bash

If fixes accumulated, create clean branch for final review.

```bash
CLEAN_BRANCH="${WORKFLOW_BRANCH}-clean"
git checkout -b "${CLEAN_BRANCH}"
git push -u origin "${CLEAN_BRANCH}"
gh pr create --base main --head "${CLEAN_BRANCH}" --title "${PR_TITLE}" --body "${PR_BODY}"
```

## Step 12: Final Merge

> **Tier**: 0 (Orchestrator) -- final merge command, no code analysis.

Tool: bash
OnFail: abort

Squash-merge and update local main.

```bash
gh pr merge "${WORKFLOW_BRANCH}-clean" --repo "${REPO}" --squash --delete-branch
git checkout main && git pull origin main
```

## ELSE

## Step 12b: Final Merge (Direct)

> **Tier**: 0 (Orchestrator) -- direct merge, no code analysis needed.

Tool: bash
OnFail: abort

First-pass clean review: merge the existing PR directly.

```bash
gh pr merge "${PR_NUM}" --repo "${REPO}" --squash --delete-branch
git checkout main && git pull origin main
```

## ENDIF

## ENDIF
