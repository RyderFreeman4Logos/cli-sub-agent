#!/usr/bin/env bash
# Verify the PATH-resolved `csa` matches the just-built artifact (issue #2686).
# Usage: scripts/verify-csa-install-provenance.sh <artifact> [target]
set -euo pipefail

artifact="${1:?artifact path required}"
target="${2:-/usr/local/bin/csa}"

hash -r 2>/dev/null || true
exec "$artifact" doctor install --artifact "$artifact" --target "$target"
