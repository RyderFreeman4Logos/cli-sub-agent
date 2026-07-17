#!/usr/bin/env bash
# Shared authoritative full-workspace quality gate entrypoint.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"
exec scripts/hooks/quality-gate-receipt.sh -- scripts/hooks/pre-push-quality-gates.sh
