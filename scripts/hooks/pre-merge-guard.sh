#!/usr/bin/env bash
# Post-pr-bot verification: confirms pr-bot completed for a given PR.
#
# Usage: pre-merge-guard.sh <PR_NUMBER>
#
# Checks for a pr-bot completion marker in the standard marker directory.
# Exit 0 if marker found (pr-bot ran successfully), exit 1 if missing.
#
# NOTE: The .done marker is written by dev2merge Step 13 AFTER pr-bot
# completes (including the merge itself). This script is therefore a
# post-pr-bot verification — it confirms that the merge was performed
# by the pr-bot workflow, not by a rogue LLM calling `gh pr merge`
# directly. Used by dev2merge Step 14 (post-merge sync) and available
# for other post-merge audit workflows.

set -euo pipefail

PR_NUMBER="${1:?Usage: pre-merge-guard.sh <PR_NUMBER>}"

# Derive repo slug: prefer gh CLI for reliability, fall back to regex.
REPO_SLUG="$(gh repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null | tr '/' '_')" || true
if [ -z "${REPO_SLUG}" ]; then
  REPO_SLUG="$(git remote get-url origin 2>/dev/null | sed -E 's#^(https?://[^/]+/|ssh://[^/]+/|[^:]+:)##; s/\.git$//' | tr '/' '_')"
fi
if [ -z "${REPO_SLUG}" ]; then
  echo "ERROR: Cannot determine repo slug from gh CLI or git origin." >&2
  exit 1
fi

MARKER_DIR="${HOME}/.local/state/cli-sub-agent/pr-bot-markers/${REPO_SLUG}"

if [ ! -d "${MARKER_DIR}" ]; then
  echo "ERROR: Marker directory does not exist: ${MARKER_DIR}" >&2
  echo "pr-bot has never been run for ${REPO_SLUG}. Run pr-bot before merging." >&2
  exit 1
fi

# Glob match by PR number — covers HEAD changes from pr-bot fix cycles.
if ls "${MARKER_DIR}/${PR_NUMBER}"-*.done 1>/dev/null 2>&1; then
  MARKER_FILE="$(ls -t "${MARKER_DIR}/${PR_NUMBER}"-*.done | head -1)"
  echo "pre-merge-guard: pr-bot completion verified for ${REPO_SLUG} PR #${PR_NUMBER}."
  echo "  Marker: ${MARKER_FILE}"
  exit 0
else
  echo "ERROR: No pr-bot completion marker found for ${REPO_SLUG} PR #${PR_NUMBER}." >&2
  echo "  Expected: ${MARKER_DIR}/${PR_NUMBER}-<HEAD_SHA>.done" >&2
  echo "Run pr-bot before merging." >&2
  exit 1
fi
