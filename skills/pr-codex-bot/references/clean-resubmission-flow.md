# Clean Resubmission Flow (Step 11)

When the bot converges after fix iterations, the PR has accumulated
incremental fix commits. Create a clean PR for audit-friendly history.

## Procedure

```bash
# 1. Create new branch from main
git checkout -b "${BRANCH}-clean" main

# 2. Squash merge all changes
git merge --squash "${BRANCH}"

# 3. Unstage for selective re-commit
git reset HEAD

# 4. Recommit in logical groups by concern
#    Use `git add <specific files>` to stage by group
#    Each commit = one logical concern (not one file)

# 5. Push new branch
git push -u origin "${BRANCH}-clean"

# 6. Create new PR linking to old one
gh pr create --title "[type](scope): [description]" \
  --body "$(cat <<'PREOF'
## Summary
[description]

## Background
Clean resubmission of #${OLD_PR_NUM}. The original PR went through
N rounds of iterative review with @codex. Fix commits have been
consolidated into logical groups here.

See #${OLD_PR_NUM} for the full review discussion.

## Test plan
- [ ] `cargo clippy -p [package] -- -D warnings`
- [ ] `cargo test -p [package]`
- [ ] @codex review
PREOF
)"

# 7. Close old PR
gh pr comment "${OLD_PR_NUM}" --repo "${REPO}" \
  --body "Superseded by #${NEW_PR_NUM}. Preserved for review discussion reference."
gh pr close "${OLD_PR_NUM}" --repo "${REPO}"

# 8. Use Baseline Capture Template (see above) for new PR
TMP_PREFIX="/tmp/codex-bot-${REPO//\//-}-${NEW_PR_NUM}"
gh api "repos/${REPO}/pulls/${NEW_PR_NUM}/comments?per_page=100" --paginate --slurp \
  --jq '[.[].[] | select(.user.login == "chatgpt-codex-connector[bot]") | .id] | sort' \
  > "${TMP_PREFIX}-baseline.json" || {
  echo "ERROR: Failed to capture PR comments baseline"
  exit 1
}
gh api "repos/${REPO}/issues/${NEW_PR_NUM}/comments?per_page=100" --paginate --slurp \
  --jq '[.[].[] | select(.user.login == "chatgpt-codex-connector[bot]") | .id] | sort' \
  > "${TMP_PREFIX}-issue-baseline.json" || {
  echo "ERROR: Failed to capture issue comments baseline"
  exit 1
}
BASELINE_REVIEW_COUNT=$(gh api "repos/${REPO}/pulls/${NEW_PR_NUM}/reviews?per_page=100" --paginate --slurp \
  --jq '[.[].[] | select(.user.login == "chatgpt-codex-connector[bot]")] | length') || {
  echo "ERROR: Failed to capture baseline review count"
  exit 1
}
echo "${BASELINE_REVIEW_COUNT}" > "${TMP_PREFIX}-review-count.txt"
gh pr comment "${NEW_PR_NUM}" --repo "${REPO}" --body "@codex review"

# 9. Update PR_NUM and reset TMP_PREFIX to match new PR
PR_NUM="${NEW_PR_NUM}"
TMP_PREFIX="/tmp/codex-bot-${REPO//\//-}-${PR_NUM}"  # Force reset after PR change
```

## Commit Grouping Strategy

Group by **concern**, not by chronology or file:

| Concern | Typical Files | Commit Convention |
|---------|--------------|-------------------|
| Core abstractions | types, mod, registry | `feat(scope): [what the types enable]` |
| Implementation | executor, engine | `feat(scope): [what the engine does]` |
| Configuration | config, schema | `feat(scope): [what becomes configurable]` |
| Integration | router, dispatch | `feat(scope): [where it's wired in]` |
| Tests | test modules | `test(scope): [what is verified]` |
| Formatting | (if needed) | `style(scope): apply cargo fmt` |

**Number of commits is flexible** — use as many as needed for logical separation.

## Preservation Policy

| Artifact | Action | Reason |
|----------|--------|--------|
| Old branch | Keep | Audit trail |
| Old commits | Keep | Shows iterative development |
| Old PR | Close with comment | Links to new PR, preserves discussion |
| New branch | Active | Clean history for merge |
| New PR | Active | Fresh review with coherent diff |
