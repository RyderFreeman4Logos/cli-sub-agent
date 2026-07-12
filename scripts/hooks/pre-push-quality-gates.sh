#!/usr/bin/env bash
# Authoritative local replacement for pull-request and branch GitHub CI.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

scripts/monolith/check.sh --scope all --baseline scripts/monolith/baseline.toml --report-all
just monolith-test
just exact-build-test
just check-path-includes
just check-version-bumped
just check-chinese
scripts/cargo-env-normalize.sh cargo fmt --all -- --check
./scripts/hooks/check-env-dependent-tests.sh
just deny
just clippy
just test
just test-e2e
