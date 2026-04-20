#!/usr/bin/env bash

set -euo pipefail

CURRENT_BRANCH="${1:-$(git branch --show-current)}"

if [ -z "${CURRENT_BRANCH}" ]; then
  echo "ERROR: cannot determine current branch for pr-bot remote resolution." >&2
  exit 1
fi

BRANCH_PUSH_REMOTE="$(git config --get "branch.${CURRENT_BRANCH}.pushRemote" 2>/dev/null || true)"
PUSH_DEFAULT_REMOTE="$(git config --get remote.pushDefault 2>/dev/null || true)"
REMOTE_COUNT="$(git remote | wc -l | tr -d ' ')"

if [ -z "${BRANCH_PUSH_REMOTE}" ] && [ -z "${PUSH_DEFAULT_REMOTE}" ] && [ "${REMOTE_COUNT}" -gt 1 ] && git remote | grep -qx origin; then
  ORIGIN_URL="$(git remote get-url --push origin 2>/dev/null || true)"
  GITHUB_LOGIN="$(gh api user --jq .login 2>/dev/null || true)"

  if [ -n "${GITHUB_LOGIN}" ] && [ -n "${ORIGIN_URL}" ] && ! printf '%s' "${ORIGIN_URL}" | grep -qiE "[:/]${GITHUB_LOGIN}/"; then
    echo "ERROR: pr-bot detected an ambiguous fork convention." >&2
    echo "  origin URL: ${ORIGIN_URL}" >&2
    echo "  authenticated GitHub login: ${GITHUB_LOGIN}" >&2
    echo "  origin does not reference your login, so it likely points at the canonical repository." >&2
    echo "  Multiple remotes exist and neither 'branch.${CURRENT_BRANCH}.pushRemote' nor 'remote.pushDefault' is configured." >&2
    echo "  Set the branch push remote explicitly:" >&2
    echo "    git config branch.${CURRENT_BRANCH}.pushRemote <your-fork-remote-name>" >&2
    echo "  Or set a global default:" >&2
    echo "    git config remote.pushDefault <your-fork-remote-name>" >&2
    echo "  Then re-run pr-bot." >&2
    exit 2
  fi
fi

REMOTE_NAME="${BRANCH_PUSH_REMOTE}"
if [ -z "${REMOTE_NAME}" ]; then
  REMOTE_NAME="${PUSH_DEFAULT_REMOTE}"
fi
if [ -z "${REMOTE_NAME}" ] && git remote | grep -qx origin; then
  REMOTE_NAME=origin
fi
if [ -z "${REMOTE_NAME}" ]; then
  REMOTE_NAME="$(git config --get "branch.${CURRENT_BRANCH}.remote" 2>/dev/null || true)"
fi
if [ -z "${REMOTE_NAME}" ]; then
  REMOTE_NAME="$(git config --get checkout.defaultRemote 2>/dev/null || true)"
fi
if [ -z "${REMOTE_NAME}" ] && [ "${REMOTE_COUNT}" = "1" ]; then
  REMOTE_NAME="$(git remote | head -1)"
fi

if [ -z "${REMOTE_NAME}" ]; then
  echo "ERROR: cannot determine target remote. Multiple remotes exist and neither" >&2
  echo "  'branch.${CURRENT_BRANCH}.pushRemote', 'remote.pushDefault'," >&2
  echo "  'branch.${CURRENT_BRANCH}.remote', 'checkout.defaultRemote', nor 'origin' is set." >&2
  echo "  Configure one with: git config --local branch.${CURRENT_BRANCH}.pushRemote <name>" >&2
  exit 1
fi

printf '%s\n' "${REMOTE_NAME}"
