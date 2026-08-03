#!/usr/bin/env bash
# Verify the workspace version differs from main on feature branches.
# This script backs `just check-version-bumped` and is also invoked directly by
# Rust tests so workspace tests do not depend on the host having `just` installed.
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

repo_root="${1:-}"
if [ -z "$repo_root" ]; then
    repo_root="$(git rev-parse --show-superproject-working-tree 2>/dev/null | grep . || git rev-parse --show-toplevel)"
fi

branch=$(git symbolic-ref --short HEAD 2>/dev/null || echo "")
if [ "$branch" = "main" ] || [ "$branch" = "" ]; then
    exit 0
fi
if [ "${CSA_SKIP_VERSION_CHECK:-0}" = "1" ]; then
    exit 0
fi

# Extract workspace version from Cargo.toml on current branch vs main.
cargo_install_root="${CARGO_INSTALL_ROOT:-}"
if [ -z "$cargo_install_root" ] || [ "$cargo_install_root" = "/usr/local" ]; then
    cargo_install_root="$repo_root/target/cargo-install-root"
fi
mkdir -p "$cargo_install_root"
current=$(CARGO_INSTALL_ROOT="$cargo_install_root" "$script_dir/cargo-env-normalize.sh" cargo metadata --no-deps --format-version 1 \
    | jq -r '.packages[] | select(.name == "cli-sub-agent") | .version')
main_version=$(git show main:Cargo.toml 2>/dev/null \
    | grep -A1 '^\[workspace\.package\]' \
    | grep '^version' | head -1 \
    | sed 's/.*"\(.*\)".*/\1/' || echo "")
if [ -z "$main_version" ]; then
    echo "WARNING: Could not read main branch version, skipping check."
    exit 0
fi
if [ "$current" = "$main_version" ]; then
    echo ""
    echo "=========================================="
    echo "ERROR: Workspace version ($current) matches main."
    echo "=========================================="
    echo ""
    echo "You must bump the version before committing on a feature branch."
    echo "Run:  just bump-patch"
    echo ""
    exit 1
fi
