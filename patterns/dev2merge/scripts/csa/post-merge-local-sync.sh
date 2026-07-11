set -euo pipefail
# NOTE: PR_NUMBER comes from Step 15 (gh pr view/list). In fork workflows,
# pr-bot may resolve a different PR via owner-aware lookup. For single-repo
# workflows (the common case), both resolve to the same PR.
if [ -n "${PR_NUMBER:-}" ]; then
  # --- Deterministic gate: verify pr-bot completion marker ---
  # Prefer exact marker path from Step 16 (CSA_VAR:PR_BOT_DONE_MARKER).
  # Fall back to repo-scoped glob if variable is unset (backwards compat).
  if [ -n "${PR_BOT_DONE_MARKER:-}" ]; then
    if [ ! -f "${PR_BOT_DONE_MARKER}" ]; then
      echo "ERROR: pr-bot marker not found: ${PR_BOT_DONE_MARKER}" >&2
      echo "Step 16 (pr-bot) must complete successfully before post-merge sync." >&2
      exit 1
    fi
    echo "pr-bot completion marker verified (exact): ${PR_BOT_DONE_MARKER}"
  else
    # Fallback: glob match by repo slug + PR number.
    # NOTE: glob may match stale markers from previous pr-bot runs on the same
    # PR. The exact CSA_VAR path (above) is the primary defense; this fallback
    # exists only for edge cases where the variable is lost.
    REPO_SLUG="$(gh repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null | tr '/' '_')" || true
    if [ -z "${REPO_SLUG}" ]; then
      REPO_SLUG="$(git remote get-url origin 2>/dev/null | sed -E 's#^(https?://[^/]+/|ssh://[^/]+/|[^:]+:)##; s/\.git$//' | tr '/' '_')"
    fi
    MARKER_DIR="${HOME}/.local/state/cli-sub-agent/pr-bot-markers/${REPO_SLUG}"
    if ! ls "${MARKER_DIR}/${PR_NUMBER}"-*.done 1>/dev/null 2>&1; then
      echo "ERROR: No pr-bot completion marker found for PR #${PR_NUMBER}." >&2
      echo "Step 16 (pr-bot) must complete successfully before post-merge sync." >&2
      echo "Marker directory: ${MARKER_DIR}" >&2
      exit 1
    fi
    echo "pr-bot completion marker verified (glob) for PR #${PR_NUMBER}."
  fi

  # --- Verify PR is actually merged (defense in depth) ---
  PR_STATE="$(gh pr view "${PR_NUMBER}" --json state -q '.state' 2>/dev/null || echo "UNKNOWN")"
  if [ "${PR_STATE}" != "MERGED" ]; then
    echo "ERROR: PR #${PR_NUMBER} state is '${PR_STATE}', expected 'MERGED'." >&2
    echo "pr-bot marker exists but PR not merged — possible partial failure." >&2
    exit 1
  fi
  echo "PR #${PR_NUMBER} confirmed MERGED."
fi
FEATURE_BRANCH="$(git branch --show-current 2>/dev/null || true)"
SYNC_REMOTE="origin"
SYNC_DEFAULT_BRANCH="${DEFAULT_BRANCH:-}"
if [ -z "${SYNC_DEFAULT_BRANCH}" ]; then
  SYNC_DEFAULT_BRANCH="$(git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's@^refs/remotes/origin/@@' || true)"
fi
if [ -z "${SYNC_DEFAULT_BRANCH}" ]; then
  echo "WARNING: post-merge checkout skipped: could not determine default branch." >&2
  exit 0
fi
if ! git checkout "${SYNC_DEFAULT_BRANCH}"; then
  echo "WARNING: post-merge checkout of ${SYNC_DEFAULT_BRANCH} failed; leaving ${FEATURE_BRANCH:-current branch} checked out." >&2
  exit 0
fi
if ! git pull --ff-only "${SYNC_REMOTE}" "${SYNC_DEFAULT_BRANCH}"; then
  echo "WARNING: post-merge pull of ${SYNC_REMOTE}/${SYNC_DEFAULT_BRANCH} failed; merge already completed." >&2
  exit 0
fi
LOCAL_SHA="$(git rev-parse HEAD)"
REMOTE_SHA="$(git rev-parse "${SYNC_REMOTE}/${SYNC_DEFAULT_BRANCH}" 2>/dev/null || true)"
if [ -n "${REMOTE_SHA}" ] && [ "${LOCAL_SHA}" != "${REMOTE_SHA}" ]; then
  echo "WARNING: Local ${SYNC_DEFAULT_BRANCH} (${LOCAL_SHA}) does not match ${SYNC_REMOTE}/${SYNC_DEFAULT_BRANCH} (${REMOTE_SHA}) after sync." >&2
  exit 0
fi
echo "Local ${SYNC_DEFAULT_BRANCH} synced to ${LOCAL_SHA}."
