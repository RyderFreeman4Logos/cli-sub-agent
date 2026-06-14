#!/usr/bin/env bash

set -euo pipefail

REMOTE_NAME="${1:-${REMOTE_NAME:-origin}}"
DEFAULT_BRANCH="${2:-${DEFAULT_BRANCH:-main}}"

if [ "${CSA_SKIP_VERSION_CHECK:-0}" = "1" ]; then
  exit 0
fi

if [ ! -f Cargo.toml ]; then
  echo "pr-bot version gate skipped: no Cargo.toml found"
  exit 0
fi

require_version_check() {
  [ "${CSA_REQUIRE_VERSION_CHECK:-0}" = "1" ]
}

has_justfile() {
  [ -f justfile ] || [ -f Justfile ] || [ -f .justfile ]
}

if ! command -v just >/dev/null 2>&1; then
  if require_version_check; then
    echo "ERROR: CSA_REQUIRE_VERSION_CHECK=1 but 'just' is unavailable; cannot run the pre-merge version gate." >&2
    echo "Install just or unset CSA_REQUIRE_VERSION_CHECK for repositories without a mandatory version gate." >&2
    exit 1
  fi
  echo "pr-bot version gate skipped: 'just' is unavailable"
  exit 0
fi

JUST_SUMMARY=""
if ! JUST_SUMMARY="$(just --summary 2>&1)"; then
  if require_version_check || has_justfile; then
    echo "ERROR: Could not inspect just targets before the pre-merge version gate." >&2
    printf '%s\n' "${JUST_SUMMARY}" >&2
    echo "Fix the justfile or unset CSA_REQUIRE_VERSION_CHECK for repositories without a mandatory version gate." >&2
    exit 1
  fi
  echo "pr-bot version gate skipped: just targets are unavailable"
  exit 0
fi

if ! printf '%s\n' "${JUST_SUMMARY}" | tr ' ' '\n' | grep -qx 'check-version-bumped'; then
  if require_version_check; then
    echo "ERROR: CSA_REQUIRE_VERSION_CHECK=1 but just target 'check-version-bumped' is unavailable." >&2
    echo "Add a local just target named 'check-version-bumped' or unset CSA_REQUIRE_VERSION_CHECK." >&2
    exit 1
  fi
  echo "pr-bot version gate skipped: just target 'check-version-bumped' is unavailable"
  exit 0
fi

if [ -z "${REMOTE_NAME}" ] || [ -z "${DEFAULT_BRANCH}" ]; then
  echo "ERROR: REMOTE_NAME and DEFAULT_BRANCH are required for the pre-merge version gate." >&2
  exit 1
fi

if ! git fetch --quiet "${REMOTE_NAME}" "refs/heads/${DEFAULT_BRANCH}:refs/heads/${DEFAULT_BRANCH}"; then
  echo "ERROR: Could not refresh ${REMOTE_NAME}/${DEFAULT_BRANCH} before the pre-merge version gate." >&2
  echo "Run:  git fetch ${REMOTE_NAME} ${DEFAULT_BRANCH}:${DEFAULT_BRANCH}" >&2
  exit 1
fi

if ! just check-version-bumped; then
  echo "" >&2
  echo "==========================================" >&2
  echo "BLOCKED: pre-merge version bump gate failed." >&2
  echo "==========================================" >&2
  echo "" >&2
  echo "The PR branch version still matches ${DEFAULT_BRANCH} after refreshing from ${REMOTE_NAME}." >&2
  echo "Run:  just bump-patch" >&2
  echo "Then rerun pr-bot so review and merge use the bumped HEAD." >&2
  echo "" >&2
  exit 1
fi
