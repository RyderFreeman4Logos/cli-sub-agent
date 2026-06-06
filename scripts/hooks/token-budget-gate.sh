#!/usr/bin/env bash
# L4 quality gate: token budget check for changed files in review range.
# Delegates to the shared monolith checker for text files in the review range.
#
# Usage: token-budget-gate.sh [range]
#   range: git diff range (default: main...HEAD)
#
# Exit codes:
#   0 = no hard failures
#   1 = new/regressed monolith debt or missing baseline metadata
set -euo pipefail

RANGE="${1:-main...HEAD}"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

scripts/monolith/check.sh \
    --scope range \
    --range "$RANGE" \
    --baseline scripts/monolith/baseline.toml \
    --report-all
