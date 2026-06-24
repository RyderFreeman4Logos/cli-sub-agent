#!/usr/bin/env bash

set -euo pipefail

CURRENT_BRANCH="${1:-$(git branch --show-current)}"

if [ -z "${CURRENT_BRANCH}" ]; then
  echo "ERROR: cannot determine current branch for pr-bot remote resolution." >&2
  exit 1
fi

BRANCH_PUSH_REMOTE="$(git config --get "branch.${CURRENT_BRANCH}.pushRemote" 2>/dev/null || true)"
PUSH_DEFAULT_REMOTE="$(git config --get remote.pushDefault 2>/dev/null || true)"
BRANCH_REMOTE="$(git config --get "branch.${CURRENT_BRANCH}.remote" 2>/dev/null || true)"
CHECKOUT_DEFAULT_REMOTE="$(git config --get checkout.defaultRemote 2>/dev/null || true)"
REMOTE_COUNT="$(git remote | wc -l | tr -d ' ')"
REMOTE_LIST="$(git remote | tr '\n' ' ' | sed 's/[[:space:]]*$//')"

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

remote_has_push_url() {
  local remote_name="$1"
  local push_url

  [ -n "${remote_name}" ] || return 1
  push_url="$(git remote get-url --push "${remote_name}" 2>/dev/null)" || return 1
  [ -n "${push_url}" ]
}

require_valid_explicit_push_remote() {
  local config_key="$1"
  local remote_name="$2"

  [ -n "${remote_name}" ] || return 0
  if remote_has_push_url "${remote_name}"; then
    return 0
  fi

  echo "ERROR: invalid explicit pr-bot push remote." >&2
  echo "  ${config_key}: ${remote_name}" >&2
  echo "  The configured remote does not exist or has no non-empty push URL." >&2
  echo "  Fix: set it to a valid push remote:" >&2
  echo "    git config ${config_key} <remote-name>" >&2
  echo "  Or unset it to allow fallback remote resolution:" >&2
  echo "    git config --unset ${config_key}" >&2
  exit 1
}

select_remote() {
  local candidate

  for candidate in \
    "${BRANCH_PUSH_REMOTE}" \
    "${PUSH_DEFAULT_REMOTE}" \
    "origin" \
    "${BRANCH_REMOTE}" \
    "${CHECKOUT_DEFAULT_REMOTE}"
  do
    if remote_has_push_url "${candidate}"; then
      printf '%s\n' "${candidate}"
      return 0
    fi
  done

  if [ "${REMOTE_COUNT}" = "1" ]; then
    candidate="$(git remote | head -1)"
    if remote_has_push_url "${candidate}"; then
      printf '%s\n' "${candidate}"
      return 0
    fi
  fi

  return 1
}

if [ -n "${BRANCH_PUSH_REMOTE}" ]; then
  require_valid_explicit_push_remote "branch.${CURRENT_BRANCH}.pushRemote" "${BRANCH_PUSH_REMOTE}"
else
  require_valid_explicit_push_remote "remote.pushDefault" "${PUSH_DEFAULT_REMOTE}"
fi

REMOTE_NAME="$(select_remote || true)"

if [ -z "${REMOTE_NAME}" ]; then
  echo "ERROR: cannot determine a non-empty pr-bot push remote with a push URL." >&2
  echo "  branch: ${CURRENT_BRANCH}" >&2
  echo "  branch.${CURRENT_BRANCH}.pushRemote: ${BRANCH_PUSH_REMOTE:-<unset>}" >&2
  echo "  remote.pushDefault: ${PUSH_DEFAULT_REMOTE:-<unset>}" >&2
  echo "  origin present: $(git remote | grep -qx origin && printf yes || printf no)" >&2
  echo "  branch.${CURRENT_BRANCH}.remote: ${BRANCH_REMOTE:-<unset>}" >&2
  echo "  checkout.defaultRemote: ${CHECKOUT_DEFAULT_REMOTE:-<unset>}" >&2
  echo "  remote count: ${REMOTE_COUNT}" >&2
  echo "  remotes: ${REMOTE_LIST:-<none>}" >&2
  echo "  Fix: set an explicit push remote with a push URL:" >&2
  echo "    git config --local branch.${CURRENT_BRANCH}.pushRemote <name>" >&2
  echo "  Or set a default push remote:" >&2
  echo "    git config remote.pushDefault <name>" >&2
  exit 1
fi

printf '%s\n' "${REMOTE_NAME}"
