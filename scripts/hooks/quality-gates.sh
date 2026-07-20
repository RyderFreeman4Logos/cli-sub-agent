#!/usr/bin/env bash
# Shared authoritative full-workspace quality gate entrypoint.
# Live mutable checks always run. Only the hermetic offline static stage may reuse
# an exact-input receipt.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"
scripts/hooks/quality-gates-live.sh
exec scripts/hooks/quality-gate-receipt.sh -- scripts/hooks/pre-push-quality-gates.sh
