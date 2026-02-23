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
debating stale comments without staleness check, trusting `reviewed=true`
without SHA verification, auto-merging or auto-aborting at round limit.

## Dispatcher Model Note

This pattern follows a 3-layer dispatcher architecture:
- **Layer 0 (Orchestrator)**: The main agent dispatches steps -- never touches code directly.
- **Layer 1 (Executors)**: CSA sub-agents and Task tool agents perform actual work.
- **Layer 2 (Sub-sub-agents)**: Spawned by Layer 1 for specific sub-tasks (invisible to Layer 0).

Each step below is annotated with its execution layer.

## Step 1: Commit Changes

> **Layer**: 0 (Orchestrator) -- lightweight shell command, no code reading.

Tool: bash

Ensure all changes committed. Set WORKFLOW_BRANCH once (persists through
clean branch switches in Step 11).

```bash
WORKFLOW_BRANCH="$(git branch --show-current)"
```

## Step 2: Local Pre-PR Review (SYNCHRONOUS — MUST NOT background)

> **Layer**: 1 (CSA executor) -- Layer 0 dispatches `csa review`, which spawns
> Layer 2 reviewer model(s) internally. Orchestrator waits for result.

Tool: bash
OnFail: abort

Run cumulative local review covering all commits since main.
This is the FOUNDATION — without it, bot unavailability cannot safely merge.

> Fast-path (SHA-verified): compare `git rev-parse HEAD` with the HEAD SHA
> stored in the latest `csa review` session metadata. If they match, skip
> Step 2 review. If they do not match (or metadata is missing), run full
> `csa review --branch main`. Any HEAD drift (including amend) auto-invalidates
> the fast-path.

```bash
CURRENT_HEAD="$(git rev-parse HEAD)"
REVIEW_HEAD="$(csa session list --recent-review 2>/dev/null | parse_head_sha || true)"
if [ -n "${REVIEW_HEAD}" ] && [ "${CURRENT_HEAD}" = "${REVIEW_HEAD}" ]; then
  echo "Fast-path: local review already covers current HEAD."
else
  csa review --branch main
fi
```

## IF ${LOCAL_REVIEW_HAS_ISSUES}

## Step 3: Fix Local Review Issues

> **Layer**: 1 (CSA executor) -- Layer 0 dispatches fix task to CSA. CSA reads
> code, applies fixes, and returns results. Orchestrator reviews outcome.

Tool: csa
Tier: tier-2-standard
OnFail: retry 3

Fix issues found by local review. Loop until clean (max 3 rounds).

## ENDIF

## Step 4: Push and Create PR

> **Layer**: 0 (Orchestrator) -- shell commands only, no code reading/writing.

Tool: bash
OnFail: abort

```bash
git push -u origin "${WORKFLOW_BRANCH}"
gh pr create --base main --title "${PR_TITLE}" --body "${PR_BODY}"
PR_NUM=$(gh pr view --json number -q '.number')
```

## Step 4a: Check Cloud Bot Configuration

> **Layer**: 0 (Orchestrator) -- config check, lightweight.

Tool: bash

Check whether cloud bot review is enabled for this project.

```bash
CLOUD_BOT=$(csa config get pr_review.cloud_bot --default true)
```

If `CLOUD_BOT` is `false`:
- Skip Steps 5 through 10 (cloud bot trigger, poll, classify, arbitrate, fix).
- Reuse the same SHA-verified fast-path before supplementary review:
  - If current `HEAD` matches latest reviewed session HEAD SHA → skip review.
  - Otherwise run full `csa review --branch main`.
- Jump directly to Step 12b (Final Merge — Direct).

## IF ${CLOUD_BOT} != "false"

## Step 5: Trigger Cloud Bot Review and Poll for Response

> **Layer**: 0 (Orchestrator) -- trigger + polling loop, no code analysis.
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
  # Fallback: run local csa review for coverage confirmation.
  # Non-zero exit means review found issues -- block merge and route to fix cycle.
  if ! csa review --range main...HEAD 2>/dev/null; then
    echo "BLOCKED: Fallback review found issues. Routing to fix cycle."
    FALLBACK_REVIEW_HAS_ISSUES=true
  fi
fi

# --- Gate: fallback review failure blocks all downstream paths ---
# This check runs unconditionally after the bot-timeout block.
# When FALLBACK_REVIEW_HAS_ISSUES=true, the orchestrator MUST route to
# Step 6-fix (the dedicated fallback fix cycle within the timeout branch).
# Steps 7-10 are in the BOT_UNAVAILABLE=false branch and NOT reachable here.
# FORBIDDEN: Reaching any merge step while FALLBACK_REVIEW_HAS_ISSUES=true.
#
# NOTE: This gate uses an output marker (FALLBACK_BLOCKED) instead of exit 1
# because Step 5 is declared OnFail: skip — a non-zero exit would be silently
# swallowed, allowing the workflow to fall through to merge. The marker is
# immune to skip-on-failure semantics: the orchestrator reads stdout and
# routes based on the signal, not the exit code.
if [ "${FALLBACK_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "FALLBACK_BLOCKED: Fallback review found issues. Entering fix cycle (Step 6-fix)."
  echo "Orchestrator MUST route to Step 6-fix (fallback fix cycle within timeout branch)."
  echo "FORBIDDEN: Falling through to any merge step from this path."
  # Exit 0 so OnFail:skip does not swallow the signal.
  # Orchestrator routes on the FALLBACK_BLOCKED marker, not exit code.
fi
```

## IF ${BOT_UNAVAILABLE}

## Step 6-fix: Fallback Review Fix Cycle (Bot Timeout Path)

> **Layer**: 1 (CSA executor) -- Layer 0 dispatches fix task to CSA. CSA reads
> code, applies fixes, and returns results. Orchestrator re-runs review.

Tool: csa
Tier: tier-2-standard
OnFail: retry 2

When `FALLBACK_REVIEW_HAS_ISSUES=true` (set in Step 5 when `csa review`
found issues during bot timeout), this dedicated fix cycle runs WITHIN the
timeout branch. Steps 7-10 are structurally inside the `BOT_UNAVAILABLE=false`
branch and are NOT reachable from here — this cycle is self-contained.

Loop: fix issues → re-run `csa review --range main...HEAD` → check result.
Max 3 rounds. If still failing after 3 rounds, abort.

```bash
FALLBACK_FIX_ROUND=0
FALLBACK_FIX_MAX=3
while [ "${FALLBACK_REVIEW_HAS_ISSUES}" = "true" ] && [ "${FALLBACK_FIX_ROUND}" -lt "${FALLBACK_FIX_MAX}" ]; do
  FALLBACK_FIX_ROUND=$((FALLBACK_FIX_ROUND + 1))
  echo "Fallback fix round ${FALLBACK_FIX_ROUND}/${FALLBACK_FIX_MAX}"

  # 1. Fix issues found by csa review (delegated to CSA executor)
  csa run "Fix the issues found by the local review (csa review --range main...HEAD). Read the review output and apply fixes. Commit the fixes."

  # 2. Re-run csa review to verify fixes
  if csa review --range main...HEAD 2>/dev/null; then
    echo "Fallback review now passes. Proceeding to merge."
    FALLBACK_REVIEW_HAS_ISSUES=false
  else
    echo "Fallback review still has issues after round ${FALLBACK_FIX_ROUND}."
  fi
done

if [ "${FALLBACK_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Fallback review still failing after ${FALLBACK_FIX_MAX} fix rounds. Aborting."
  exit 1
fi
```

## Step 6a: Merge Without Bot

> **Layer**: 0 (Orchestrator) -- merge command, no code analysis.

Tool: bash

Bot unavailable. Local fallback review passed (either initially in Step 5,
or after fix cycle in Step 6-fix). Step 6-fix guarantees
`FALLBACK_REVIEW_HAS_ISSUES=false` before reaching this point.

```bash
# --- Hard gate: unconditional pre-merge check ---
if [ "${FALLBACK_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved fallback review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi
if [ "${REBASE_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved post-rebase review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi

# Push fallback fix commits so the remote PR head includes them.
# Without this, gh pr merge uses the stale remote HEAD and omits fixes.
git push origin "${WORKFLOW_BRANCH}"

gh pr merge "${PR_NUM}" --repo "${REPO}" --squash --delete-branch
git checkout main && git pull origin main
```

## ELSE

## IF ${BOT_HAS_ISSUES}

## Step 7: Evaluate Each Bot Comment

> **Layer**: 1 (claude-code / Task tool) -- Layer 0 dispatches comment
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

> **Layer**: 1 (CSA debate) -- Layer 0 dispatches to `csa debate`, which
> internally spawns Layer 2 independent models for adversarial evaluation.
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

> **Layer**: 1 (CSA executor) -- Layer 0 dispatches fix to CSA sub-agent.
> CSA reads code, applies fix, commits. Orchestrator verifies result.

Tool: csa
Tier: tier-2-standard
OnFail: retry 2

Fix the real issue (non-stale, non-false-positive). Commit the fix.

## ENDIF

## ENDFOR

## Step 10: Push Fixes and Re-trigger

> **Layer**: 0 (Orchestrator) -- shell commands to push and re-trigger bot.

Tool: bash

Track iteration count via `REVIEW_ROUND`. Check the round cap BEFORE
pushing fixes and triggering another bot review. When `REVIEW_ROUND`
reaches `MAX_REVIEW_ROUNDS` (default: 10), STOP and present options to
the user — no new review is triggered until the user decides:

- **Option A**: Merge now (review is good enough)
- **Option B**: Continue for `MAX_REVIEW_ROUNDS` more rounds
- **Option C**: Abort and investigate manually

The workflow MUST NOT auto-merge or auto-abort at the round limit.
The user MUST explicitly choose an option before proceeding.

**Orchestrator protocol**: When the round cap is hit, the bash block exits
with code 0 after printing `ROUND_LIMIT_HALT`. The orchestrator (Layer 0)
MUST then use `AskUserQuestion` to present options A/B/C and collect the
user's choice. Based on the answer, set `ROUND_LIMIT_ACTION` and re-enter
this step. The action handler at the TOP of the script processes the user's
choice BEFORE the round cap check, so the chosen action always takes effect:
- **A**: Set `ROUND_LIMIT_ACTION=merge` → clears `ROUND_LIMIT_REACHED`, prints `ROUND_LIMIT_MERGE`, exits 0. Orchestrator routes to Step 12/12b.
- **B**: Set `ROUND_LIMIT_ACTION=continue` → clears `ROUND_LIMIT_REACHED`, extends `MAX_REVIEW_ROUNDS`, falls through to push/re-trigger.
- **C**: Set `ROUND_LIMIT_ACTION=abort` → leaves `ROUND_LIMIT_REACHED=true`, prints `ROUND_LIMIT_ABORT`, exits 1.

**CRITICAL**: The `merge` and `continue` branches MUST clear `ROUND_LIMIT_REACHED=false`
before proceeding. Steps 10.5, 11, and 12 are gated by `!(${ROUND_LIMIT_REACHED})`,
so a stale `true` value blocks all downstream merge/rebase paths even after the user
explicitly chose to proceed. The `abort` branch intentionally leaves the flag set,
as it halts the workflow.

**Signal disambiguation**: The orchestrator distinguishes re-entry outcomes by
output markers, NOT exit codes alone. `ROUND_LIMIT_HALT` (exit 0) = ask user.
`ROUND_LIMIT_MERGE` (exit 0) = proceed to merge. `ROUND_LIMIT_ABORT` (exit 1) = stop.

```bash
REVIEW_ROUND=$((REVIEW_ROUND + 1))
MAX_REVIEW_ROUNDS="${MAX_REVIEW_ROUNDS:-10}"

# --- Handle orchestrator re-entry with user decision (FIRST) ---
# When the orchestrator re-enters after collecting user choice via
# AskUserQuestion, ROUND_LIMIT_ACTION is set. Process it BEFORE the round
# cap check so the user's choice always takes effect regardless of round count.
#
# CRITICAL: The merge path prints ROUND_LIMIT_MERGE (distinct from
# ROUND_LIMIT_HALT) so the orchestrator can unambiguously route to Step 12/12b.
# The abort path exits non-zero. The continue path falls through to push/trigger.
if [ -n "${ROUND_LIMIT_ACTION}" ]; then
  case "${ROUND_LIMIT_ACTION}" in
    merge)
      echo "User chose: Merge now. Pushing local commits before merge."
      ROUND_LIMIT_REACHED=false  # Clear so Steps 10.5/11/12 are unblocked
      # Push any Category C fixes from Step 9 so remote HEAD includes them.
      # Without this, gh pr merge merges the stale remote head.
      git push origin "${WORKFLOW_BRANCH}"
      echo "ROUND_LIMIT_MERGE: Routing to merge step."
      # Orchestrator MUST route to Step 12/12b upon seeing ROUND_LIMIT_MERGE.
      # Distinct from ROUND_LIMIT_HALT — this is an affirmative merge decision.
      exit 0
      ;;
    continue)
      echo "User chose: Continue. Extending by ${MAX_REVIEW_ROUNDS} rounds."
      ROUND_LIMIT_REACHED=false  # Clear so review loop and downstream steps are unblocked
      MAX_REVIEW_ROUNDS=$((REVIEW_ROUND + MAX_REVIEW_ROUNDS))
      unset ROUND_LIMIT_ACTION
      # Fall through to push/re-trigger below (bypasses round cap check)
      ;;
    abort)
      echo "User chose: Abort workflow."
      echo "ROUND_LIMIT_ABORT: Workflow terminated by user."
      exit 1
      ;;
  esac
fi

# --- Round cap check BEFORE push/trigger ---
# This block ONLY fires when ROUND_LIMIT_ACTION is unset (first hit, or after
# continue already extended the cap). When ROUND_LIMIT_ACTION was set, the case
# block above already handled it and either exited or fell through past this check.
if [ "${REVIEW_ROUND}" -ge "${MAX_REVIEW_ROUNDS}" ]; then
  echo "Reached MAX_REVIEW_ROUNDS (${MAX_REVIEW_ROUNDS})."
  echo "Options:"
  echo "  A) Merge now (review is good enough)"
  echo "  B) Continue for ${MAX_REVIEW_ROUNDS} more rounds"
  echo "  C) Abort and investigate manually"
  echo ""
  echo "ROUND_LIMIT_HALT: Awaiting user decision."
  # HALT: The orchestrator MUST use AskUserQuestion to collect user's choice.
  # The shell script block ENDS here. The orchestrator handles routing based on
  # the user's answer OUTSIDE this script block. This ensures non-interactive
  # execution environments (CSA sub-agents) do not hang on stdin.
  #
  # Orchestrator routing logic (executed at Layer 0, NOT in this bash block):
  #   User answers "A" → set ROUND_LIMIT_ACTION=merge, re-enter this step
  #   User answers "B" → set ROUND_LIMIT_ACTION=continue, re-enter this step
  #   User answers "C" → set ROUND_LIMIT_ACTION=abort, re-enter this step
  #
  # FORBIDDEN: Falling through to push/trigger without a user decision.
  exit 0  # Yield control to orchestrator for AskUserQuestion
fi

# --- Push fixes and re-trigger bot review ---
git push origin "${WORKFLOW_BRANCH}"
gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review"
```

Loop back to Step 5 (poll).

## ELSE

## Step 10a: Bot Review Clean

No issues found by bot. Proceed to Step 10.5 (rebase) then merge.

## Step 10.5: Rebase for Clean History

> **Layer**: 0 (Orchestrator) -- git history cleanup before merge.

Tool: bash

When the branch has accumulated fix commits from review iterations,
reorganize them into logical groups before merging.

**Skip this step if**:
- The branch has <= 3 commits (already clean enough)
- All commits already follow a logical grouping

```bash
COMMIT_COUNT=$(git rev-list --count main..HEAD)
if [ "${COMMIT_COUNT}" -gt 3 ]; then
  # 1. Create backup branch (idempotent: -f overwrites if exists from prior run)
  git branch -f "backup-${PR_NUM}-pre-rebase" HEAD

  # 2. Soft reset to merge-base (not local main tip, which may have advanced)
  MERGE_BASE=$(git merge-base main HEAD)
  git reset --soft $MERGE_BASE

  # 3. Create logical commits by selectively staging files per phase/concern
  #    After soft reset, ALL changes are staged in the index. We must unstage
  #    everything first, then selectively re-stage and commit per logical group.
  #
  #    The orchestrator (Layer 0) delegates this to a Layer 1 executor:
  #    a) Unstage all changes: git reset HEAD .
  #    b) Discover changed files dynamically via git diff
  #    c) For each logical group: git add <files> && git commit
  #    d) Verify all changes are committed: git status --porcelain is empty
  #
  #    Dynamic grouping: discover actual changed paths instead of hard-coding
  #    directory names (which fail with pathspec errors on repos without them).
  git reset HEAD .
  CHANGED_FILES=$(git diff --name-only HEAD)

  # Group 1: Source code (any file under directories containing code)
  SOURCE_FILES=$(echo "${CHANGED_FILES}" | grep -E '^(src/|crates/|lib/|bin/)' || true)
  if [ -n "${SOURCE_FILES}" ]; then
    echo "${SOURCE_FILES}" | xargs git add --
    if ! git diff --cached --quiet; then
      git commit -m "feat(scope): primary implementation changes"
    fi
  fi

  # Group 2: Patterns, skills, and workflow definitions
  PATTERN_FILES=$(echo "${CHANGED_FILES}" | grep -E '^(patterns/|\.claude/)' || true)
  if [ -n "${PATTERN_FILES}" ]; then
    echo "${PATTERN_FILES}" | xargs git add --
    if ! git diff --cached --quiet; then
      git commit -m "fix(scope): pattern and skill updates"
    fi
  fi

  # Group 3: Everything else (config, docs, tests, etc.)
  git add -A
  if ! git diff --cached --quiet; then
    git commit -m "chore(scope): config and documentation updates"
  fi
  #
  #    IMPORTANT: Each commit is guarded by `git diff --cached --quiet` to skip
  #    empty groups without halting the script. Groups are discovered dynamically
  #    from actual changed files, so repos without src/ or crates/ directories
  #    will not trigger pathspec errors.

  # 4. Verify replacement commits exist before force pushing
  NEW_COMMIT_COUNT=$(git rev-list --count ${MERGE_BASE}..HEAD)
  if [ "${NEW_COMMIT_COUNT}" -eq 0 ]; then
    echo "ERROR: No replacement commits created after soft reset. Aborting push."
    echo "Restoring from backup branch."
    git reset --hard "backup-${PR_NUM}-pre-rebase"
    exit 1
  fi

  # 5. Force push
  git push --force-with-lease

  # 6. Trigger one final @codex review to verify rebased code
  gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review"

  # 7. Poll for bot response (reuse Step 5 polling logic)
  REBASE_BOT_OK=false
  POLL_INTERVAL=30
  MAX_WAIT=600
  WAITED=0
  while [ "${WAITED}" -lt "${MAX_WAIT}" ]; do
    sleep "${POLL_INTERVAL}"
    WAITED=$((WAITED + POLL_INTERVAL))
    BOT_REPLY=$(gh api "repos/${REPO}/issues/${PR_NUM}/comments" \
      --jq "[.[] | select(.user.type == \"Bot\" or .user.login == \"codex[bot]\" or .user.login == \"codex-bot\") | select(.created_at > \"$(git log -1 --format=%cI HEAD)\")] | length" 2>/dev/null || echo "0")
    if [ "${BOT_REPLY}" -gt 0 ] 2>/dev/null; then
      REBASE_BOT_OK=true
      break
    fi
    echo "Post-rebase poll... ${WAITED}s / ${MAX_WAIT}s"
  done

  # 8. BLOCKING: Evaluate final review result before merge
  #    The orchestrator MUST NOT proceed to merge until this gate passes.
  if [ "${REBASE_BOT_OK}" = "true" ]; then
    echo "Post-rebase review received. Evaluating..."
    # Orchestrator classifies the final bot response using Step 7 logic.
    # Extract bot comments posted after the force-push and check for actionable issues.
    #
    # Detection uses P0/P1/P2 badge presence (e.g., "**P0**", "**P1**", "**P2**") instead of
    # raw keyword grep. The bot always emits P0/P1/P2 severity badges for real issues;
    # keyword matching ("issue|error|fix|warning|problem") misclassifies clean
    # summaries like "No issues found" because they contain "issue".
    REBASE_BOT_ISSUES=$(gh api "repos/${REPO}/issues/${PR_NUM}/comments" \
      --jq "[.[] | select(.user.type == \"Bot\" or .user.login == \"codex[bot]\" or .user.login == \"codex-bot\") | select(.created_at > \"$(git log -1 --format=%cI HEAD)\") | select(.body | test(\"\\*\\*P[012]\\*\\*\"))] | length" 2>/dev/null || echo "0")

    if [ "${REBASE_BOT_ISSUES}" -gt 0 ] 2>/dev/null; then
      echo "BLOCKED: Post-rebase review found ${REBASE_BOT_ISSUES} actionable comment(s)."
      echo "Routing to inline fix cycle. Merge is blocked."
      REBASE_REVIEW_HAS_ISSUES=true
      # NOTE: We do NOT set BOT_HAS_ISSUES=true here because we are already
      # past the BOT_HAS_ISSUES branch point — setting it would have no effect
      # on control flow. Instead, a dedicated fix cycle runs inline below.
      # FORBIDDEN: Falling through to merge from this path.

      # --- Inline post-rebase fix cycle ---
      REBASE_FIX_ROUND=0
      REBASE_FIX_MAX=3
      while [ "${REBASE_REVIEW_HAS_ISSUES}" = "true" ] && [ "${REBASE_FIX_ROUND}" -lt "${REBASE_FIX_MAX}" ]; do
        REBASE_FIX_ROUND=$((REBASE_FIX_ROUND + 1))
        echo "Post-rebase fix round ${REBASE_FIX_ROUND}/${REBASE_FIX_MAX}"

        # 1. Fix issues found by post-rebase bot review
        csa run "Fix the issues found by the post-rebase bot review on PR #${PR_NUM}. Read the bot comments and apply fixes. Commit the fixes."

        # 2. Push fixes and re-trigger bot review
        git push origin "${WORKFLOW_BRANCH}"
        gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review"

        # 3. Poll for new bot response
        REFIX_BOT_OK=false
        REFIX_WAITED=0
        while [ "${REFIX_WAITED}" -lt "${MAX_WAIT}" ]; do
          sleep "${POLL_INTERVAL}"
          REFIX_WAITED=$((REFIX_WAITED + POLL_INTERVAL))
          REFIX_REPLY=$(gh api "repos/${REPO}/issues/${PR_NUM}/comments" \
            --jq "[.[] | select(.user.type == \"Bot\" or .user.login == \"codex[bot]\" or .user.login == \"codex-bot\") | select(.created_at > \"$(git log -1 --format=%cI HEAD)\")] | length" 2>/dev/null || echo "0")
          if [ "${REFIX_REPLY}" -gt 0 ] 2>/dev/null; then
            REFIX_BOT_OK=true
            break
          fi
        done

        # 4. Evaluate result
        if [ "${REFIX_BOT_OK}" = "true" ]; then
          REFIX_ISSUES=$(gh api "repos/${REPO}/issues/${PR_NUM}/comments" \
            --jq "[.[] | select(.user.type == \"Bot\" or .user.login == \"codex[bot]\" or .user.login == \"codex-bot\") | select(.created_at > \"$(git log -1 --format=%cI HEAD)\") | select(.body | test(\"\\*\\*P[012]\\*\\*\"))] | length" 2>/dev/null || echo "0")
          if [ "${REFIX_ISSUES}" -eq 0 ] 2>/dev/null; then
            echo "Post-rebase review now passes after fix round ${REBASE_FIX_ROUND}."
            REBASE_REVIEW_HAS_ISSUES=false
          else
            echo "Post-rebase review still has ${REFIX_ISSUES} issue(s) after round ${REBASE_FIX_ROUND}."
          fi
        else
          # Bot timed out during fix cycle — fall back to local review
          if csa review --range main...HEAD 2>/dev/null; then
            echo "Local fallback review passes after fix round ${REBASE_FIX_ROUND}."
            REBASE_REVIEW_HAS_ISSUES=false
          else
            echo "Local fallback review still has issues after round ${REBASE_FIX_ROUND}."
          fi
        fi
      done

      if [ "${REBASE_REVIEW_HAS_ISSUES}" = "true" ]; then
        echo "ERROR: Post-rebase review still failing after ${REBASE_FIX_MAX} fix rounds. Aborting."
        exit 1
      fi
      echo "REBASE_FIXED: Post-rebase issues resolved. Proceeding to merge."
    else
      echo "Post-rebase review is clean. Proceeding to merge."
      REBASE_REVIEW_HAS_ISSUES=false
      # Fall through to merge (Step 12/12b).
    fi
  else
    echo "Post-rebase bot timed out. Falling back to local review."
    if ! csa review --range main...HEAD 2>/dev/null; then
      echo "BLOCKED: Post-rebase fallback review found issues."
      FALLBACK_REVIEW_HAS_ISSUES=true
    fi
    # Gate: fallback review failure blocks merge, routes to inline fix cycle.
    # This check is unconditional — runs whether csa review passed or failed.
    if [ "${FALLBACK_REVIEW_HAS_ISSUES}" = "true" ]; then
      # NOTE: We do NOT set BOT_HAS_ISSUES=true here because we are already
      # past the BOT_HAS_ISSUES branch point — setting it would have no effect.
      # Instead, a dedicated fix cycle runs inline below.

      # --- Inline post-rebase fallback fix cycle ---
      REBASE_FB_FIX_ROUND=0
      REBASE_FB_FIX_MAX=3
      while [ "${FALLBACK_REVIEW_HAS_ISSUES}" = "true" ] && [ "${REBASE_FB_FIX_ROUND}" -lt "${REBASE_FB_FIX_MAX}" ]; do
        REBASE_FB_FIX_ROUND=$((REBASE_FB_FIX_ROUND + 1))
        echo "Post-rebase fallback fix round ${REBASE_FB_FIX_ROUND}/${REBASE_FB_FIX_MAX}"

        # 1. Fix issues found by local review
        csa run "Fix the issues found by csa review --range main...HEAD. Read the review output and apply fixes. Commit the fixes."

        # 2. Re-run local review to verify fixes
        if csa review --range main...HEAD 2>/dev/null; then
          echo "Post-rebase fallback review now passes after fix round ${REBASE_FB_FIX_ROUND}."
          FALLBACK_REVIEW_HAS_ISSUES=false
        else
          echo "Post-rebase fallback review still has issues after round ${REBASE_FB_FIX_ROUND}."
        fi
      done

      if [ "${FALLBACK_REVIEW_HAS_ISSUES}" = "true" ]; then
        echo "ERROR: Post-rebase fallback review still failing after ${REBASE_FB_FIX_MAX} fix rounds. Aborting."
        exit 1
      fi
      # Push fallback fix commits so remote PR head includes them.
      # Without this, gh pr merge merges stale remote HEAD and drops fixes.
      git push origin "${WORKFLOW_BRANCH}"
      echo "REBASE_FALLBACK_FIXED: Post-rebase fallback issues resolved. Proceeding to merge."
    fi
  fi
fi
```

## ENDIF

## ENDIF

## ENDIF
<!-- End of CLOUD_BOT != "false" block -->

## IF !(${BOT_UNAVAILABLE})

## IF ${FIXES_ACCUMULATED}

## Step 11: Clean Resubmission (if needed)

> **Layer**: 0 (Orchestrator) -- git branch management, no code reading.

Tool: bash

If fixes accumulated, create clean branch for final review.

```bash
CLEAN_BRANCH="${WORKFLOW_BRANCH}-clean"
git checkout -b "${CLEAN_BRANCH}"
git push -u origin "${CLEAN_BRANCH}"
gh pr create --base main --head "${CLEAN_BRANCH}" --title "${PR_TITLE}" --body "${PR_BODY}"
```

## Step 12: Final Merge

> **Layer**: 0 (Orchestrator) -- final merge command, no code analysis.

Tool: bash
OnFail: abort

Squash-merge and update local main.

```bash
# --- Hard gate: unconditional pre-merge check ---
if [ "${FALLBACK_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved fallback review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi
if [ "${REBASE_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved post-rebase review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi

gh pr merge "${WORKFLOW_BRANCH}-clean" --repo "${REPO}" --squash --delete-branch
git checkout main && git pull origin main
```

## ELSE

## Step 12b: Final Merge (Direct)

> **Layer**: 0 (Orchestrator) -- direct merge, no code analysis needed.

Tool: bash
OnFail: abort

First-pass clean review: merge the existing PR directly.

```bash
# --- Hard gate: unconditional pre-merge check ---
if [ "${FALLBACK_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved fallback review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi
if [ "${REBASE_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved post-rebase review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi

gh pr merge "${PR_NUM}" --repo "${REPO}" --squash --delete-branch
git checkout main && git pull origin main
```

## ENDIF

## ENDIF
