#!/usr/bin/env bash
# Git pre-push hook: verify that a csa review has been run on the current HEAD.
#
# Install: ln -sf ../../scripts/hooks/pre-push .git/hooks/pre-push
#
# This hook prevents pushing code that hasn't been reviewed by csa.
# It checks for a review session recorded for the current branch and HEAD.
# Supports both Git-only and colocated jj+git repositories.

set -euo pipefail

if [ "${CSA_SKIP_REVIEW_CHECK:-0}" = "1" ]; then
  timestamp="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  head_sha="$(git rev-parse HEAD 2>/dev/null || echo "<unknown-head>")"
  author_email="$(git config user.email 2>/dev/null || echo "<unknown-email>")"
  raw_reason="${CSA_SKIP_REVIEW_CHECK_REASON:-<unspecified>}"
  reason="$(
    printf '%s' "${raw_reason}" \
      | tr '\r\n\t' '   ' \
      | sed -E 's/[[:space:]]+/ /g; s/^ //; s/ $//'
  )"
  [ -z "${reason}" ] && reason="<unspecified>"

  mkdir -p .csa
  printf '%s %s %s %s\n' "${timestamp}" "${head_sha}" "${author_email}" "${reason}" >> .csa/review-bypass.log
  echo "WARNING: review-check bypassed via CSA_SKIP_REVIEW_CHECK=1 for ${head_sha:0:11}; logged to .csa/review-bypass.log. Reason: ${reason}" >&2
  exit 0
fi

# Skip if not in a csa-managed project
if ! command -v csa >/dev/null 2>&1; then
  exit 0
fi

CURRENT_HEAD="$(git rev-parse HEAD)"
CURRENT_BRANCH="$(git branch --show-current)"

# For colocated jj repos, also capture jj change_id
JJ_CHANGE_ID=""
if command -v jj >/dev/null 2>&1 && [ -d ".jj" ]; then
  JJ_CHANGE_ID="$(jj log --no-graph -r @ -T change_id 2>/dev/null || true)"
fi

# Skip for main/dev branches (direct pushes are blocked by branch protection)
if [ "${CURRENT_BRANCH}" = "main" ] || [ "${CURRENT_BRANCH}" = "dev" ]; then
  exit 0
fi

REVIEW_DESCRIPTION_PATTERN='^review(\[[0-9]+\])?: '

# Query review sessions for the current branch.
# Match against commit_id (Git SHA) OR change_id (jj logical ID).
REVIEW_SESSION_HEAD=""
LATEST_BRANCH_REVIEW=""
if csa session list --format json >/dev/null 2>&1; then
  REVIEW_SESSION_HEAD="$(
    csa session list --format json 2>/dev/null \
      | jq -r --arg branch "${CURRENT_BRANCH}" \
              --arg head "${CURRENT_HEAD}" \
              --arg jj_cid "${JJ_CHANGE_ID}" \
              --arg review_pattern "${REVIEW_DESCRIPTION_PATTERN}" '
          [
            .[]
            | select((.description // "") | test($review_pattern))
            | select((.branch // "") == $branch)
            | select(
                # Match by commit_id in vcs_identity (v2 sessions)
                (.vcs_identity.commit_id // "") == $head
                # OR by change_id for legacy sessions
                or (.change_id // "") == $head
                # OR by jj change_id if available
                or ($jj_cid != "" and ((.vcs_identity.change_id // "") == $jj_cid))
                or ($jj_cid != "" and ((.change_id // "") == $jj_cid))
              )
            | .session_id
          ]
          | first // empty
        ' 2>/dev/null || true
  )"
  LATEST_BRANCH_REVIEW="$(
    csa session list --format json 2>/dev/null \
      | jq -r --arg branch "${CURRENT_BRANCH}" --arg review_pattern "${REVIEW_DESCRIPTION_PATTERN}" '
          [
            .[]
            | select((.description // "") | test($review_pattern))
            | select((.branch // "") == $branch)
            | .change_id
          ]
          | first // empty
        ' 2>/dev/null || true
  )"
fi

if [ -n "${REVIEW_SESSION_HEAD}" ]; then
  echo "pre-push: Review verified for HEAD ${CURRENT_HEAD:0:11}."
  exit 0
fi

if [ -n "${LATEST_BRANCH_REVIEW}" ]; then
  echo "ERROR: Push blocked — latest recorded review for ${CURRENT_BRANCH} is ${LATEST_BRANCH_REVIEW:0:11}, not ${CURRENT_HEAD:0:11}." >&2
else
  echo "ERROR: Push blocked — no csa review session recorded for ${CURRENT_BRANCH} at HEAD ${CURRENT_HEAD:0:11}." >&2
fi
echo "Run 'csa review --range main...HEAD' before pushing." >&2
exit 1
