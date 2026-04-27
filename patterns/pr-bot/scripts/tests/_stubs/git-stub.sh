#!/usr/bin/env bash

set -euo pipefail

state_dir="${GIT_STUB_STATE_DIR:?}"
source_owner="${GIT_STUB_SOURCE_OWNER:-test-owner}"

if [ "${1:-}" = "ls-remote" ] && [ "${2:-}" = "--heads" ]; then
  if [ "${GIT_STUB_BRANCH_PUSHED:-false}" = "true" ]; then
    printf '0000000000000000000000000000000000000000\trefs/heads/%s\n' "${4:-}"
  fi
  exit 0
fi

if [ "${1:-}" = "push" ]; then
  count_file="${state_dir}/push-count"
  count=0
  if [ -f "${count_file}" ]; then
    count="$(<"${count_file}")"
  fi
  count=$((count + 1))
  printf '%s\n' "${count}" >"${count_file}"
  exit 0
fi

if [ "${1:-}" = "remote" ] && [ "${2:-}" = "get-url" ] && [ "${3:-}" = "--push" ]; then
  printf 'git@github.com:%s/test-repo.git\n' "${source_owner}"
  exit 0
fi

echo "unexpected git invocation: $*" >&2
exit 1
