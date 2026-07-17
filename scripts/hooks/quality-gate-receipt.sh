#!/usr/bin/env bash
# Reuse a full quality gate only for identical normalized acceptance inputs.
set -euo pipefail

if [ "${1:-}" != "--" ] || [ "$#" -lt 2 ]; then
  echo 'usage: quality-gate-receipt.sh -- <quality-gate-command> [args...]' >&2
  exit 2
fi
shift

repo_root="$(git rev-parse --show-toplevel)"
repo_root="$(realpath -e "$repo_root")"
cd "$repo_root"
# Local helper imports must not create an untracked cache that invalidates the
# clean-worktree manifest they are collecting.
exec env PYTHONDONTWRITEBYTECODE=1 \
  python3 scripts/quality-gate-state.py run --repo "$repo_root" -- "$@"
