# Step 10.5: Rebase for Clean History (Detail)

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
    if csa review --range main...HEAD 2>/dev/null; then
      # Audit trail: explain why merging without post-rebase bot review.
      gh pr comment "${PR_NUM}" --repo "${REPO}" --body \
        "**Merge rationale**: Post-rebase cloud bot timed out. Local \`csa review --range main...HEAD\` passed CLEAN. Proceeding to merge with local review as the review layer."
    else
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

      # Audit trail: explain why merging without post-rebase bot review.
      gh pr comment "${PR_NUM}" --repo "${REPO}" --body \
        "**Merge rationale**: Post-rebase cloud bot timed out. Local \`csa review --range main...HEAD\` passed CLEAN after ${REBASE_FB_FIX_ROUND} fix round(s). Proceeding to merge with local review as the review layer."
      echo "REBASE_FALLBACK_FIXED: Post-rebase fallback issues resolved. Proceeding to merge."
    fi
  fi
fi
```
