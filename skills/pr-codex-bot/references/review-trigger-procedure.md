# Review Trigger Procedure (Single Entry Point)

**All review triggers (Steps 4, 9, 11/12) MUST use this unified procedure.**
It handles cloud `@codex review`, polling with timeout, quota detection,
and local CSA fallback. This prevents duplicating baseline/poll/fallback
logic across multiple steps.

## Normalized Review Outcomes

| Outcome | Meaning | Next Action |
|---------|---------|-------------|
| `CLEAN` | No issues found | Proceed to merge path |
| `HAS_ISSUES` | Reviewer found issues | Proceed to Step 7 (evaluate) |
| `UNAVAILABLE(reason)` | Cloud bot did not respond | Per `fallback.cloud_review_exhausted` policy |

**Note**: The poll loop produces an intermediate result `NEW_COMMENTS_DETECTED`
which means the bot responded but the main agent must still evaluate the
response (Step 7) to determine if the final outcome is `CLEAN` or `HAS_ISSUES`.
This is not a bug — the procedure intentionally defers classification to the
agent because the bot's response format varies (inline comments, review-level
approval, issue comments).

## Phase 1: Check Fallback State

The fallback marker uses `WORKFLOW_BRANCH` (the original branch the workflow
started on, set once in Step 1 and never re-derived). This ensures the marker
persists even when Step 11 creates clean branches (`${BRANCH}-clean`,
`${BRANCH}-clean-2`, etc.) — the marker path stays the same because
`WORKFLOW_BRANCH` doesn't change. A new workflow on a different branch
starts fresh.

**CRITICAL**: `WORKFLOW_BRANCH` MUST be set once at workflow start (Step 1)
and passed unchanged through all steps. Do NOT re-derive it from
`git branch --show-current` after Step 11 branch switches.

```bash
TMP_PREFIX="${TMP_PREFIX:-/tmp/codex-bot-${REPO//\//-}-${PR_NUM}}"
# Marker uses WORKFLOW_BRANCH (original branch, not current) for cross-PR persistence
FALLBACK_MARKER="/tmp/codex-bot-${REPO//\//-}-${WORKFLOW_BRANCH//\//-}-cloud-fallback.marker"

if [ -f "${FALLBACK_MARKER}" ]; then
  echo "CLOUD_FALLBACK_ACTIVE: Skipping cloud @codex review, using local CSA review"
  # → Skip Phase 2 entirely, go directly to Phase 3 (Local Fallback Path)
  # The main agent should run: csa review --branch main
  # Then map output to CLEAN or HAS_ISSUES and proceed to Step 7
else
  # → Continue to Phase 2 (Cloud Path)
  :
fi
```

## Phase 2: Cloud Path (Baseline + Trigger + Poll)

### 2a. Baseline Capture

Capture before triggering review to prevent race conditions:

```bash
gh api "repos/${REPO}/pulls/${PR_NUM}/comments?per_page=100" --paginate --slurp \
  --jq '[.[].[] | select(.user.login == "chatgpt-codex-connector[bot]") | .id] | sort' \
  > "${TMP_PREFIX}-baseline.json" || {
  echo "ERROR: Failed to capture PR comments baseline"
  exit 1
}
gh api "repos/${REPO}/issues/${PR_NUM}/comments?per_page=100" --paginate --slurp \
  --jq '[.[].[] | select(.user.login == "chatgpt-codex-connector[bot]") | .id] | sort' \
  > "${TMP_PREFIX}-issue-baseline.json" || {
  echo "ERROR: Failed to capture issue comments baseline"
  exit 1
}
BASELINE_REVIEW_COUNT=$(gh api "repos/${REPO}/pulls/${PR_NUM}/reviews?per_page=100" --paginate --slurp \
  --jq '[.[].[] | select(.user.login == "chatgpt-codex-connector[bot]")] | length') || {
  echo "ERROR: Failed to capture baseline review count"
  exit 1
}
case "${BASELINE_REVIEW_COUNT}" in
  ''|*[!0-9]*) echo "ERROR: Invalid BASELINE_REVIEW_COUNT: ${BASELINE_REVIEW_COUNT}"; exit 1 ;;
esac
echo "${BASELINE_REVIEW_COUNT}" > "${TMP_PREFIX}-review-count.txt"
```

### 2b. Trigger Cloud Review

```bash
gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review" || {
  echo "ERROR: Failed to trigger @codex review. Check PR access and bot installation."
  exit 1
}
```

### 2c. Poll with Timeout

Max 10 minutes, bounded API error retry:

```bash
# Verify baseline files exist before polling
for f in "${TMP_PREFIX}-baseline.json" "${TMP_PREFIX}-issue-baseline.json" "${TMP_PREFIX}-review-count.txt"; do
  [ -f "$f" ] || { echo "ERROR: Missing baseline file: $f"; exit 1; }
done
BASELINE_REVIEW_COUNT="$(cat "${TMP_PREFIX}-review-count.txt")"
case "${BASELINE_REVIEW_COUNT}" in
  ''|*[!0-9]*) echo "ERROR: Invalid BASELINE_REVIEW_COUNT: ${BASELINE_REVIEW_COUNT}"; exit 1 ;;
esac
MAX_POLLS=13          # 13 * 45s ≈ 10 minutes
API_FAIL_LIMIT=5      # Max consecutive API failures before escalation
POLL_COUNT=0
API_FAIL_COUNT=0
REVIEW_RESULT=""

while [ "$POLL_COUNT" -lt "$MAX_POLLS" ]; do
  sleep 45
  POLL_COUNT=$((POLL_COUNT + 1))

  # Check PR review comments (inline code comments)
  CURRENT=$(gh api "repos/${REPO}/pulls/${PR_NUM}/comments?per_page=100" --paginate --slurp \
    --jq '[.[].[] | select(.user.login == "chatgpt-codex-connector[bot]") | .id] | sort') || {
    API_FAIL_COUNT=$((API_FAIL_COUNT + 1))
    if [ "$API_FAIL_COUNT" -ge "$API_FAIL_LIMIT" ]; then
      REVIEW_RESULT="UNAVAILABLE(api_error)"
      break
    fi
    continue
  }
  API_FAIL_COUNT=0  # Reset on success
  BASELINE=$(cat "${TMP_PREFIX}-baseline.json")
  if [ "$CURRENT" != "$BASELINE" ]; then
    REVIEW_RESULT="NEW_COMMENTS_DETECTED"
    break
  fi

  # Check issue-level comments (general PR comments — bot's primary channel)
  ISSUE_CURRENT=$(gh api "repos/${REPO}/issues/${PR_NUM}/comments?per_page=100" --paginate --slurp \
    --jq '[.[].[] | select(.user.login == "chatgpt-codex-connector[bot]") | .id] | sort') || {
    API_FAIL_COUNT=$((API_FAIL_COUNT + 1))
    if [ "$API_FAIL_COUNT" -ge "$API_FAIL_LIMIT" ]; then
      REVIEW_RESULT="UNAVAILABLE(api_error)"
      break
    fi
    continue
  }
  API_FAIL_COUNT=0
  ISSUE_BASELINE=$(cat "${TMP_PREFIX}-issue-baseline.json")
  if [ "$ISSUE_CURRENT" != "$ISSUE_BASELINE" ]; then
    REVIEW_RESULT="NEW_COMMENTS_DETECTED"
    break
  fi

  # Check for new reviews (compare count against baseline)
  CURRENT_REVIEW_COUNT=$(gh api "repos/${REPO}/pulls/${PR_NUM}/reviews?per_page=100" --paginate --slurp \
    --jq '[.[].[] | select(.user.login == "chatgpt-codex-connector[bot]")] | length') || {
    API_FAIL_COUNT=$((API_FAIL_COUNT + 1))
    if [ "$API_FAIL_COUNT" -ge "$API_FAIL_LIMIT" ]; then
      REVIEW_RESULT="UNAVAILABLE(api_error)"
      break
    fi
    continue
  }
  API_FAIL_COUNT=0
  if [ "$CURRENT_REVIEW_COUNT" -gt "$BASELINE_REVIEW_COUNT" ] 2>/dev/null; then
    REVIEW_RESULT="NEW_COMMENTS_DETECTED"
    break
  fi
done

# Timeout reached without response
if [ -z "$REVIEW_RESULT" ]; then
  REVIEW_RESULT="UNAVAILABLE(timeout)"
fi
```

### 2d. Classify Result

- `NEW_COMMENTS_DETECTED` → Check for quota message in bot reply. If bot reply
  (scoped to `chatgpt-codex-connector[bot]` user only) matches known quota
  patterns, classify as `UNAVAILABLE(quota)`. Otherwise proceed to Step 7
  evaluation which will classify as `CLEAN` or `HAS_ISSUES`.

  **Known quota message patterns** (observed from production):
  - `"You have reached your Codex usage limits for code reviews"`
  - `"add credits to your account and enable them for code reviews"`
  - Generic keywords: `"quota exceeded"`, `"subscription required"`,
    `"rate limit"`, `"usage limits"`, `"add credits"`
- `UNAVAILABLE(timeout)` → Cloud bot did not respond within 10 minutes.
- `UNAVAILABLE(api_error)` → GitHub API failed 5+ consecutive times.

### 2e. Handle UNAVAILABLE

Check the fallback policy before prompting:

```bash
# Use --global to prevent project config from overriding user's global safety policy
FALLBACK_POLICY=$(csa config get fallback.cloud_review_exhausted --global --default "ask-user")
```

**Behavior by policy:**

| Policy | Action |
|--------|--------|
| `auto-local` | Log reason, automatically fall back to local CSA review (still reviews, just locally) |
| `ask-user` | Notify user and ask for confirmation (default) |

**If `ask-user` (default):** Notify user with the specific reason and ask for
confirmation before switching to local CSA review.

```
UNAVAILABLE detected:
  Reason: [timeout | quota | api_error]

  Options:
  1. Fall back to local CSA review for remainder of this workflow
  2. Retry cloud @codex review (reset poll timer)
```

**If `auto-local`:** Log the reason and proceed directly to local fallback.
Both policies still perform a review — `auto-local` just skips the user prompt.

When falling back (either auto or user-confirmed):
```bash
echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) reason=${REASON}" > "${FALLBACK_MARKER}"
```

Then proceed to Phase 3 (Local Fallback Path).

## Phase 3: Local Fallback Path

When `${FALLBACK_MARKER}` exists (cloud unavailable for this workflow):

Run local CSA review using the `csa-review` skill with the same scope as the
cloud bot would review:

```bash
# Same scope as cloud bot: all committed changes since main
csa review --branch main  # Reviews committed changes vs main (not just uncommitted)
```

**Map CSA output to normalized outcomes**:
- CSA finds zero issues → `CLEAN`
- CSA finds one or more issues → `HAS_ISSUES` (treat each CSA finding like
  a bot comment — proceed to Step 7 evaluation with the same Category A/B/C
  classification)

**Key difference from cloud bot**: Local CSA output is structured text (not
GitHub PR comments). The main agent reads the CSA output directly instead of
polling GitHub APIs. Step 7 evaluation applies the same logic — classify each
finding, queue false positives for Step 8 arbitration, fix real issues.

**Next workflow resets**: The fallback marker is scoped to `${WORKFLOW_BRANCH}`.
A new workflow on a different branch starts fresh with cloud `@codex review`.
Within the same branch family (including Step 11 clean PRs), the fallback
state persists so you don't wait another 10 minutes for a known-unavailable bot.

## Fallback State Diagram

```
@codex review triggered
       |
       v
  Poll (max 10 min)
       |
       +── Bot responds ──> Evaluate response
       |                         |
       |                    +── Quota message? ──> UNAVAILABLE(quota)
       |                    |
       |                    +── Real review ──> CLEAN or HAS_ISSUES
       |
       +── Timeout (10 min) ──> UNAVAILABLE(timeout)
       |
       +── API errors (5x) ──> UNAVAILABLE(api_error)
       |
       v
  UNAVAILABLE:
  Check fallback.cloud_review_exhausted policy (--global)
       |
       +── auto-local ──> Fallback immediately (still reviews)
       |
       +── ask-user (default) ──> Notify user
       |         |
       |         +── User confirms fallback
       |         |         |
       |         |         v
       |         |    Create ${FALLBACK_MARKER} (WORKFLOW_BRANCH-scoped)
       |         |    Run local CSA review (--branch main)
       |         |    (all subsequent reviews in this workflow use local)
       |         |
       |         +── User retries cloud ──> Reset poll timer, try again
```

## Limitations

- **Per-workflow-branch, not per-session**: Fallback marker uses
  `${WORKFLOW_BRANCH}` (set once at workflow start) so it persists across
  PRs and clean branches within the same workflow. Concurrent workflows on
  the same original branch would share state. Acceptable for single-user usage.
- **Old markers accumulate**: `/tmp` files are not auto-cleaned between workflows.
  They are cleaned on system reboot. Non-critical for low-frequency usage.
- **10 min timeout may be short for large PRs**: The bot may take longer for
  very large diffs. User confirmation provides an escape hatch to retry.
- **Approximate timing**: `MAX_POLLS=13 * sleep 45` is approximately 10 minutes.
  Actual wall time may vary due to API call latency.
