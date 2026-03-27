#!/usr/bin/env bash
# Pre-push: verify version bumped vs main before pushing feature branches.
set -euo pipefail

branch=$(git symbolic-ref --short HEAD 2>/dev/null || echo "")
[ -z "$branch" ] && exit 0
[ "$branch" = "main" ] && exit 0
[ "$branch" = "dev" ] && exit 0
[ "${CSA_SKIP_VERSION_CHECK:-0}" = "1" ] && exit 0

current=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
main_ver=$(git show main:Cargo.toml 2>/dev/null \
    | grep -A1 '^\[workspace\.package\]' \
    | grep '^version' | head -1 \
    | sed 's/.*"\(.*\)".*/\1/' || echo "")

if [ -z "$main_ver" ]; then
    exit 0
fi

if [ "$current" = "$main_ver" ]; then
    echo ""
    echo "=========================================="
    echo "BLOCKED: Version ($current) matches main."
    echo "=========================================="
    echo ""
    echo "Run:  just bump-patch"
    echo ""
    exit 1
fi
