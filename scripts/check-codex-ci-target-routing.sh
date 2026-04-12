#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checkout_root="$(git -C "$script_dir/.." rev-parse --show-toplevel)"
scratch_parent="$checkout_root/.tmp/check-codex-ci-target-routing"
mkdir -p "$scratch_parent"

repo_root="$(mktemp -d "$scratch_parent/repo.XXXXXX")"
trap 'chmod -R u+w "$repo_root" 2>/dev/null || true; rm -rf "$repo_root"' EXIT

target_dir="$repo_root/target"
fallback_target="$repo_root/.tmp/codex-ci/target"

expect_eq() {
    local expected="$1"
    local actual="$2"
    local message="$3"

    if [ "$actual" != "$expected" ]; then
        echo "ERROR: $message" >&2
        echo "expected: $expected" >&2
        echo "actual:   $actual" >&2
        exit 1
    fi
}

local_target="$(env -u CSA_FS_SANDBOXED CODEX_CI=1 scripts/resolve-codex-ci-path.sh target "$repo_root")"
expect_eq "$target_dir" "$local_target" "local CODEX_CI shell should keep using ./target when writable"

sandbox_target="$(CODEX_CI=1 CSA_FS_SANDBOXED=1 scripts/resolve-codex-ci-path.sh target "$repo_root")"
expect_eq "$fallback_target" "$sandbox_target" "sandboxed CODEX_CI shell should use the repo-local fallback target"
