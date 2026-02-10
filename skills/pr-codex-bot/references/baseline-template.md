# Baseline Capture Template

**Use this template before every `@codex review` trigger to prevent race conditions:**

```bash
# Ensure TMP_PREFIX is set (use PR_NUM for the target PR)
TMP_PREFIX="/tmp/codex-bot-${REPO//\//-}-${PR_NUM}"

# Bot login used in jq filters below:
CODEX_BOT_LOGIN="chatgpt-codex-connector[bot]"

# Capture baseline BEFORE triggering bot review
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
echo "${BASELINE_REVIEW_COUNT}" > "${TMP_PREFIX}-review-count.txt"

# NOW trigger bot review (after baseline is captured)
gh pr comment "${PR_NUM}" --repo "${REPO}" --body "@codex review"
```

**Referenced in:** Steps 4, 9, 11

## Temp File Naming Convention

All temp files are namespaced by `${REPO}` and `${PR_NUM}` to prevent
collisions when multiple projects use this skill concurrently:

```
/tmp/codex-bot-${REPO//\//-}-${PR_NUM}-baseline.json          # PR review comments (inline)
/tmp/codex-bot-${REPO//\//-}-${PR_NUM}-issue-baseline.json    # Issue-level comments (general)
/tmp/codex-bot-${REPO//\//-}-${PR_NUM}-poll-result.txt
/tmp/codex-bot-${REPO//\//-}-${PR_NUM}-watch.sh
```

Example for `user/repo` PR `3`: `/tmp/codex-bot-user-repo-3-baseline.json`

**IMPORTANT**: The bot primarily posts to **issue-level comments** (`issues/{pr}/comments`),
not PR review comments (`pulls/{pr}/comments`). Both endpoints must be polled.
