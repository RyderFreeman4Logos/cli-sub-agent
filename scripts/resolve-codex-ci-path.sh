#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "usage: $0 <root|target> [repo_root]" >&2
    exit 2
}

mode="${1:-}"
if [ -z "$mode" ]; then
    usage
fi
if [ "$#" -gt 2 ]; then
    usage
fi

case "$mode" in
    root|target) ;;
    *)
        usage
        ;;
esac

repo_root="${2:-$(git rev-parse --show-superproject-working-tree 2>/dev/null | grep . || git rev-parse --show-toplevel)}"
target_dir="$repo_root/target"
fallback_root="$repo_root/.tmp/codex-ci"
fallback_target="$fallback_root/target"

use_fallback=false
if [ "${CODEX_CI:-0}" = "1" ]; then
    # The real codex sandbox marks itself explicitly, so local CODEX_CI shells
    # can keep using ./target while the sandbox path still gets its fallback.
    if [ "${CSA_FS_SANDBOXED:-0}" = "1" ]; then
        use_fallback=true
    fi
fi

case "$mode" in
    root)
        if [ "$use_fallback" = true ]; then
            mkdir -p "$fallback_root"
            printf '%s' "$fallback_root"
        fi
        ;;
    target)
        if [ "$use_fallback" = true ]; then
            mkdir -p "$fallback_target"
            printf '%s' "$fallback_target"
        else
            printf '%s' "$target_dir"
        fi
        ;;
esac
