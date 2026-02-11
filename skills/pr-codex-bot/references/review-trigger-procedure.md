# Review Trigger Procedure (Single Entry Point)

**All review triggers (Steps 4, 9, 11/12) MUST use this unified procedure.**
It handles cloud `@codex review`, polling with timeout, and quota detection.
This prevents duplicating baseline/poll logic across multiple steps.

## Normalized Review Outcomes

| Outcome | Meaning | Next Action |
|---------|---------|-------------|
| `CLEAN` | No issues found | Proceed to merge path |
| `HAS_ISSUES` | Reviewer found issues | Proceed to Step 7 (evaluate) |
| `UNAVAILABLE(quota)` | Cloud bot quota exhausted | **Merge directly** — local review already covers `main...HEAD` |
| `UNAVAILABLE(timeout)` | Cloud bot did not respond in 10 min | **Merge directly** — local review already covers `main...HEAD` |
| `ESCALATE(api_error)` | GitHub API failed (transient) | **Notify user** — bot may still be reviewing |

**Note**: The poll loop produces an intermediate result `NEW_COMMENTS_DETECTED`
which means the bot responded but the main agent must still evaluate the
response (Step 7) to determine if the final outcome is `CLEAN` or `HAS_ISSUES`.
This is not a bug — the procedure intentionally defers classification to the
agent because the bot's response format varies (inline comments, review-level
approval, issue comments).

## PREREQUISITE: LOCAL_REVIEW_MARKER Must Match Current HEAD

**Before this procedure can allow direct merge on UNAVAILABLE, the
LOCAL_REVIEW_MARKER MUST exist and match the current HEAD.** This is initially
set in Step 2 (pre-PR local review) and MUST be refreshed in Step 9 after
each fix cycle (re-run local review, update marker).

The direct-merge-on-UNAVAILABLE behavior is ONLY safe because the local review
has already covered the full `main...HEAD` scope at the current HEAD.

If LOCAL_REVIEW_MARKER is missing or stale (HEAD moved since last local review),
direct merge is FORBIDDEN. Re-run local review first.

## Phase 1: Cloud Path (Baseline + Trigger + Poll)

### 1a. Baseline Capture

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

### 1b. Trigger Cloud Review

```bash
gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review" || {
  echo "ERROR: Failed to trigger @codex review. Check PR access and bot installation."
  exit 1
}
```

### 1c. Poll with Timeout

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
      REVIEW_RESULT="ESCALATE(api_error)"
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
      REVIEW_RESULT="ESCALATE(api_error)"
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
      REVIEW_RESULT="ESCALATE(api_error)"
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

### 1d. Classify Result

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
- `ESCALATE(api_error)` → GitHub API failed 5+ consecutive times. Unlike quota/timeout,
  this is a transient failure — the bot may still be processing. Escalate to user.

## Phase 2: Handle Results

### UNAVAILABLE(quota) or UNAVAILABLE(timeout) — Direct Merge

When the cloud bot is deterministically unavailable (quota exhausted, or timed out),
**verify the local review marker and merge directly**.

**Rationale**: Step 2 (pre-PR local review) already reviews the FULL `main...HEAD`
range — the exact same scope the cloud bot would review. The cloud bot is an
*additional* layer of review, not the *only* layer. When it's deterministically
unavailable, the local review has already provided independent coverage.

```bash
# UNAVAILABLE(quota/timeout) — verify marker, then merge directly
LOCAL_REVIEW_MARKER="/tmp/codex-local-review-${REPO//\//-}-${BRANCH//\//-}.marker"
if [ ! -f "${LOCAL_REVIEW_MARKER}" ] || [ "$(cat "${LOCAL_REVIEW_MARKER}")" != "$(git rev-parse HEAD)" ]; then
  echo "ERROR: LOCAL_REVIEW_MARKER missing or stale. Cannot direct-merge."
  echo "Re-run local review (csa review --branch main) and refresh marker before merging."
  exit 1
fi

echo "Cloud bot UNAVAILABLE (reason: ${REVIEW_RESULT}). Merging directly."
echo "LOCAL_REVIEW_MARKER verified: local review covers main...HEAD at current HEAD."

gh pr merge "${PR_NUM}" --repo "${REPO}" --squash --delete-branch
git checkout main && git pull origin main
```

**No fallback marker needed.** No user prompt needed. No local CSA fallback review
needed. The pre-PR local review IS the safety net.

### ESCALATE(api_error) — Notify User

API errors are transient (network issues, GitHub outages, permission problems).
The bot may still be processing the review — we just can't read the result.

```
ESCALATE(api_error) detected:
  GitHub API failed 5 consecutive times.
  The bot may still be reviewing — we cannot confirm either way.

  Options:
  1. Wait and retry polling (reset timer)
  2. Merge directly (if you're confident local review is sufficient)
  3. Cancel and investigate API issues
```

**Do NOT auto-merge on api_error.** This is fundamentally different from
quota/timeout: we don't know if the bot has findings we can't see.

## Flow Diagram

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
       +── API errors (5x) ──> ESCALATE(api_error) → notify user
       |
       v
  UNAVAILABLE(quota/timeout):
  Verify LOCAL_REVIEW_MARKER matches HEAD
       |
       +── Marker valid → Merge directly ✅
       |
       +── Marker stale → Re-run local review first
```

## Limitations

- **10 min timeout may be short for large PRs**: The bot may take longer for
  very large diffs. However, since the local review already provides coverage,
  we merge directly instead of waiting indefinitely.
- **Approximate timing**: `MAX_POLLS=13 * sleep 45` is approximately 10 minutes.
  Actual wall time may vary due to API call latency.
- **Cloud bot is additive**: The cloud bot provides a SECOND independent review
  from a different model family. When it's available, it adds value. When it's
  not, the pre-PR local review is sufficient.
- **Marker freshness**: After Step 9 fixes, HEAD moves. The marker MUST be
  refreshed (re-run `csa review --branch main`) before direct merge is safe.
