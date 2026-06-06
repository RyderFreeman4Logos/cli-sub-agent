#!/usr/bin/env bash
# Git pre-push hook: verify csa review has been run on current HEAD.
# Installed by: csa setup review-gate
#
# The gate ALWAYS validates the session verdict via `csa review --check-verdict`.
# A SHA-pinned marker (.csa/state/review-gate/<branch_safe>-<short_sha>.pass) may exist
#   from a prior run; it is informational only (logged, never a standalone pass), so a
#   stale or forged marker cannot satisfy the gate without a recorded passing review.

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

# CSA-managed executors run their own review gates in the workflow. Skipping
# here prevents pre-push from recursively spawning csa review inside csa.
CSA_DEPTH_VALUE="${CSA_DEPTH:-0}"
if [ -n "${CSA_SESSION_ID:-}" ] || [[ "${CSA_DEPTH_VALUE}" =~ ^[0-9]+$ && "${CSA_DEPTH_VALUE}" -gt 0 ]]; then
  echo "pre-push: Review gate skipped inside CSA executor session; CSA workflow owns review enforcement."
  exit 0
fi

# Skip if csa is not installed in this repo
if ! command -v csa >/dev/null 2>&1; then
  exit 0
fi

CURRENT_HEAD="$(git rev-parse HEAD)"
CURRENT_BRANCH="$(git branch --show-current)"

# Skip protected branches here; pre-push branch-protection blocks them first.
PROTECTED="main dev master"
for pb in $PROTECTED; do
  if [ "${CURRENT_BRANCH}" = "$pb" ]; then
    exit 0
  fi
done

# ── Fast path: SHA-pinned marker file ────────────────────────────────────────
# Sanitize branch name the same way review_gate::sanitize_branch does:
#   '/' → '__', any non-[a-zA-Z0-9._-] → '_'
_sanitize_branch() {
  printf '%s' "$1" \
    | sed 's|/|__|g' \
    | sed 's|[^a-zA-Z0-9._-]|_|g'
}

SHORT_SHA="${CURRENT_HEAD:0:11}"
SAFE_BRANCH="$(_sanitize_branch "${CURRENT_BRANCH}")"
MARKER=".csa/state/review-gate/${SAFE_BRANCH}-${SHORT_SHA}.pass"

if [ -f "${MARKER}" ]; then
  echo "pre-push: Review gate marker found for ${SAFE_BRANCH} at ${SHORT_SHA}; validating session."
fi

# ── Session-store validation ─────────────────────────────────────────────────
if csa review --check-verdict; then
  echo "pre-push: Full-diff review verified for HEAD ${SHORT_SHA}."
  exit 0
fi

# ── Blocked — emit reverse prompt injection for agent context ─────────────────
cat >&2 <<GATE_BLOCKED
<!-- CSA:REVIEW_GATE_BLOCKED branch="${SAFE_BRANCH}" head_sha="${CURRENT_HEAD}" -->
Push blocked: no passing review found for current HEAD.
Run: csa review --range main...HEAD --sa-mode true
Wait for PASS verdict, then retry push.
<!-- /CSA:REVIEW_GATE_BLOCKED -->
GATE_BLOCKED

echo "" >&2
echo "ERROR: Push blocked — no PASS/CLEAN full-diff csa review session recorded for ${SAFE_BRANCH} at ${SHORT_SHA}." >&2
exit 1
