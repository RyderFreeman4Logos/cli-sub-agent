set -euo pipefail
if [ -z "${PR_NUMBER:-}" ]; then
  echo "ERROR: PR_NUMBER not set — Step 15 must run first." >&2
  exit 1
fi
HEAD_SHA="$(git rev-parse --verify HEAD)"

# --- Lock + Idempotency: skip if pr-bot already ran or is running ---
# Bind markers to repo identity to prevent cross-repo PR# collisions.
REPO_SLUG="$(gh repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null | tr '/' '_')" || true
if [ -z "${REPO_SLUG}" ]; then
  REPO_SLUG="$(git remote get-url origin 2>/dev/null | sed -E 's#^(https?://[^/]+/|ssh://[^/]+/|[^:]+:)##; s/\.git$//' | tr '/' '_')"
fi
MARKER_DIR="${HOME}/.local/state/cli-sub-agent/pr-bot-markers/${REPO_SLUG}"
mkdir -p "${MARKER_DIR}"
MARKER_BASE="${MARKER_DIR}/${PR_NUMBER}-${HEAD_SHA}"
DONE_MARKER="${MARKER_BASE}.done"
LOCK_DIR="${MARKER_BASE}.lock"
LOCK_HELD=0

cleanup_lock() {
  if [ "${LOCK_HELD}" -eq 1 ]; then
    rmdir "${LOCK_DIR}" 2>/dev/null || true
  fi
}
trap cleanup_lock EXIT

if [ -f "${DONE_MARKER}" ]; then
  echo "pr-bot already completed for PR #${PR_NUMBER} at HEAD ${HEAD_SHA:0:11}; skipping."
  echo "CSA_VAR:PR_BOT_DONE_MARKER=${DONE_MARKER}"
  echo '<!-- CSA:NEXT_STEP cmd="post-merge local sync (Step 17)" required=true -->'
elif ! mkdir "${LOCK_DIR}" 2>/dev/null; then
  echo "ERROR: pr-bot already running for PR #${PR_NUMBER} at HEAD ${HEAD_SHA:0:11}." >&2
  echo "Wait for the other run to finish, or remove the lock: ${LOCK_DIR}" >&2
  exit 1
else
  LOCK_HELD=1
  echo "Running pr-bot for PR #${PR_NUMBER} (${PR_URL:-unknown})..."
  export CSA_PR_BOT_GUARD=1
  if csa plan run --sa-mode true patterns/pr-bot/workflow.toml; then
    touch "${DONE_MARKER}"
    echo "CSA_VAR:PR_BOT_DONE_MARKER=${DONE_MARKER}"
    echo '<!-- CSA:NEXT_STEP cmd="post-merge local sync (Step 17)" required=true -->'
    LOCK_HELD=0
    rmdir "${LOCK_DIR}" 2>/dev/null || true
  else
    echo "ERROR: pr-bot workflow failed for PR #${PR_NUMBER}." >&2
    exit 1
  fi
fi
