#!/usr/bin/env bash
# Hermetic, network-free static portion of the authoritative local quality gate.
# Mutable policy, credentials, network state, and ignored executables belong in
# quality-gates-live.sh and must never be covered by a reusable receipt.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

just monolith-test
just exact-build-test
just check-chinese
scripts/cargo-env-normalize.sh cargo fmt --all -- --check
just clippy
# `just test` already includes workspace e2e tests in both default and
# all-feature runs; do not execute the same all-feature e2e binary a third time.
NEXTEST_PROFILE=static NEXTEST_USER_CONFIG_FILE=none NEXTEST_RETRIES=0 just test
