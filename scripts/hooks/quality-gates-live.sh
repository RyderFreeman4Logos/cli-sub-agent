#!/usr/bin/env bash
# Mutable/current quality checks that must execute on every direct or hook call.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

scripts/monolith/check.sh --scope all --baseline scripts/monolith/baseline.toml --report-all
just check-path-includes
if [ "${CSA_QUALITY_GATE_HOOK_MODE:-0}" != "1" ]; then
  just check-version-bumped
fi
./scripts/hooks/check-env-dependent-tests.sh
scripts/hooks/quality-gate-contract-tests.sh
# Advisory databases and policy/network freshness are deliberately live. They
# are never authenticated by, or skipped because of, the static-stage receipt.
just deny
