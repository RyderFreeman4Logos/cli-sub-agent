#!/usr/bin/env bash
# Git pre-push hook: verify that a csa review has been run on the current HEAD.
#
# Install: ln -sf ../../scripts/hooks/pre-push .git/hooks/pre-push
#
# This hook prevents pushing code that hasn't been reviewed by csa.
# It checks for a review session recorded for the current branch and HEAD.
# Supports both Git-only and colocated jj+git repositories.

set -euo pipefail

# Auto-bypass when running inside a nested CSA session (depth > 0).
# Rationale: parent orchestrator reviews the final SHA before the user push
# (pr-bot Step 2 cumulative local review + Step 8 fix re-review). Employee
# push attempts should NOT trigger a nested csa review — that duplicates the
# audit the parent already runs, burning ~1M tokens per duplicate session
# (issue #890). The bypass is logged below with a distinguishable reason
# starting with "nested CSA session" so audit trail is preserved.
if [ -z "${CSA_SKIP_REVIEW_CHECK:-}" ] && [ "${CSA_DEPTH:-0}" -gt 0 ]; then
  CSA_SKIP_REVIEW_CHECK=1
  CSA_SKIP_REVIEW_CHECK_REASON="nested CSA session (depth=${CSA_DEPTH}, sid=${CSA_SESSION_ID:-<unknown>}); parent will review final SHA"
fi

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

# Skip for main/dev branches (direct pushes are blocked by branch protection)
if [ "${CURRENT_BRANCH}" = "main" ] || [ "${CURRENT_BRANCH}" = "dev" ]; then
  exit 0
fi

if csa review --check-verdict; then
  echo "pre-push: Full-diff review verified for HEAD ${CURRENT_HEAD:0:11}."
  exit 0
fi

echo "ERROR: Push blocked — no PASS/CLEAN full-diff csa review session recorded for ${CURRENT_BRANCH} at HEAD ${CURRENT_HEAD:0:11}." >&2
echo "Run 'csa review --range main...HEAD' before pushing." >&2
exit 1
