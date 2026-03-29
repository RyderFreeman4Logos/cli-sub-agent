#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/hooks/post-pr-create.sh [--base <branch>] [--pr-number <number>]

Confirms that the current feature branch has an open PR, then runs the
pr-bot workflow as a synchronous post-create transaction.
EOF
}

BASE_BRANCH="main"
REQUESTED_PR_NUMBER=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --base)
      shift
      if [ "$#" -eq 0 ]; then
        echo "ERROR: Missing value for --base." >&2
        exit 1
      fi
      BASE_BRANCH="$1"
      ;;
    --pr-number)
      shift
      if [ "$#" -eq 0 ]; then
        echo "ERROR: Missing value for --pr-number." >&2
        exit 1
      fi
      REQUESTED_PR_NUMBER="$1"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "ERROR: Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
  shift
done

if ! command -v gh >/dev/null 2>&1; then
  echo "ERROR: gh is required for post-pr-create transaction." >&2
  exit 1
fi

CURRENT_BRANCH="$(git branch --show-current 2>/dev/null || true)"
if [ -z "${CURRENT_BRANCH}" ] || [ "${CURRENT_BRANCH}" = "HEAD" ]; then
  echo "ERROR: Cannot determine current branch." >&2
  exit 1
fi

DEFAULT_BRANCH="$(git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's@^refs/remotes/origin/@@')"
if [ -z "${DEFAULT_BRANCH}" ]; then
  DEFAULT_BRANCH="main"
fi

if [ "${CURRENT_BRANCH}" = "${DEFAULT_BRANCH}" ] || [ "${CURRENT_BRANCH}" = "dev" ]; then
  echo "ERROR: post-pr-create must run from a feature branch, not ${CURRENT_BRANCH}." >&2
  exit 1
fi

if [ "${BASE_BRANCH}" != "main" ]; then
  echo "ERROR: post-pr-create currently supports base branch 'main' only." >&2
  exit 1
fi

resolve_current_pr() {
  local pr_view pr_list pr_count

  if pr_view="$(gh pr view --json number,url,headRefName,baseRefName,state 2>/dev/null)"; then
    if [ "$(printf '%s' "${pr_view}" | jq -r '.headRefName')" = "${CURRENT_BRANCH}" ] \
      && [ "$(printf '%s' "${pr_view}" | jq -r '.baseRefName')" = "${BASE_BRANCH}" ] \
      && [ "$(printf '%s' "${pr_view}" | jq -r '.state')" = "OPEN" ]; then
      printf '%s\n' "${pr_view}"
      return 0
    fi
  fi

  pr_list="$(
    gh pr list --state open --base "${BASE_BRANCH}" --head "${CURRENT_BRANCH}" \
      --json number,url,headRefName,baseRefName,state 2>/dev/null || true
  )"
  pr_count="$(printf '%s' "${pr_list}" | jq 'length')"

  if [ "${pr_count}" = "1" ]; then
    printf '%s\n' "${pr_list}" | jq '.[0]'
    return 0
  fi

  if [ "${pr_count}" = "0" ]; then
    return 1
  fi

  echo "ERROR: Multiple open PRs found for branch ${CURRENT_BRANCH} targeting ${BASE_BRANCH}." >&2
  return 2
}

resolve_requested_pr() {
  local pr_view

  if [ -z "${REQUESTED_PR_NUMBER}" ]; then
    return 1
  fi

  if ! printf '%s' "${REQUESTED_PR_NUMBER}" | grep -Eq '^[0-9]+$'; then
    echo "ERROR: --pr-number must be numeric, got '${REQUESTED_PR_NUMBER}'." >&2
    return 2
  fi

  pr_view="$(
    gh pr view "${REQUESTED_PR_NUMBER}" --json number,url,headRefName,baseRefName,state \
      2>/dev/null || true
  )"
  if [ -z "${pr_view}" ]; then
    return 1
  fi

  if [ "$(printf '%s' "${pr_view}" | jq -r '.headRefName')" = "${CURRENT_BRANCH}" ] \
    && [ "$(printf '%s' "${pr_view}" | jq -r '.baseRefName')" = "${BASE_BRANCH}" ] \
    && [ "$(printf '%s' "${pr_view}" | jq -r '.state')" = "OPEN" ]; then
    printf '%s\n' "${pr_view}"
    return 0
  fi

  echo "ERROR: PR #${REQUESTED_PR_NUMBER} is not OPEN on ${CURRENT_BRANCH} -> ${BASE_BRANCH}." >&2
  return 2
}

resolve_marker_paths() {
  local pr_number="$1"
  local head_sha="$2"
  local repo_slug
  repo_slug="$(gh repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null | tr '/' '_')"
  if [ -z "${repo_slug}" ]; then
    repo_slug="$(git remote get-url origin 2>/dev/null | sed -E 's#^(https?://[^/]+/|ssh://[^/]+/|[^:]+:)##; s/\.git$//' | tr '/' '_')"
  fi
  PR_BOT_MARKER_DIR="${HOME}/.local/state/cli-sub-agent/pr-bot-markers/${repo_slug}"
  PR_BOT_MARKER_BASE="${PR_BOT_MARKER_DIR}/${pr_number}-${head_sha}"
  PR_BOT_DONE_MARKER="${PR_BOT_MARKER_BASE}.done"
  PR_BOT_AWAITING_USER_MARKER="${PR_BOT_MARKER_BASE}.awaiting-user"
  PR_BOT_LOCK_DIR="${PR_BOT_MARKER_BASE}.lock"
}

begin_pr_bot_transaction() {
  mkdir -p "${PR_BOT_MARKER_DIR}"

  if [ -f "${PR_BOT_DONE_MARKER}" ]; then
    echo "pr-bot already completed for PR #${PR_NUMBER} at HEAD ${HEAD_SHA}; skipping."
    return 1
  fi

  if [ -f "${PR_BOT_AWAITING_USER_MARKER}" ]; then
    echo "pr-bot is awaiting manual follow-up for PR #${PR_NUMBER} at HEAD ${HEAD_SHA}; skipping auto rerun."
    return 1
  fi

  if ! mkdir "${PR_BOT_LOCK_DIR}" 2>/dev/null; then
    echo "pr-bot already running for PR #${PR_NUMBER} at HEAD ${HEAD_SHA}; skipping."
    return 1
  fi

  PR_BOT_LOCK_HELD=1
  trap 'if [ "${PR_BOT_LOCK_HELD:-0}" -eq 1 ]; then rmdir "${PR_BOT_LOCK_DIR}" 2>/dev/null || true; fi' EXIT
  return 0
}

PR_JSON=""
if [ -n "${REQUESTED_PR_NUMBER}" ]; then
  set +e
  PR_JSON="$(resolve_requested_pr)"
  rc=$?
  set -e

  if [ "${rc}" -eq 2 ]; then
    exit 1
  fi
fi

if [ -z "${PR_JSON}" ]; then
  for attempt in 1 2 3 4 5; do
    set +e
    PR_JSON="$(resolve_current_pr)"
    rc=$?
    set -e

    if [ "${rc}" -eq 0 ]; then
      break
    fi

    if [ "${rc}" -eq 2 ]; then
      exit 1
    fi

    if [ "${attempt}" -lt 5 ]; then
      sleep 2
    fi
  done
fi

if [ -z "${PR_JSON}" ]; then
  echo "ERROR: No open PR found for branch ${CURRENT_BRANCH} targeting ${BASE_BRANCH}." >&2
  exit 1
fi

PR_NUMBER="$(printf '%s' "${PR_JSON}" | jq -r '.number')"
PR_URL="$(printf '%s' "${PR_JSON}" | jq -r '.url')"

if ! printf '%s' "${PR_NUMBER}" | grep -Eq '^[0-9]+$'; then
  echo "ERROR: Failed to resolve a numeric PR number for ${CURRENT_BRANCH}." >&2
  exit 1
fi

HEAD_SHA="$(git rev-parse --verify HEAD 2>/dev/null || true)"
if [ -z "${HEAD_SHA}" ]; then
  echo "ERROR: Failed to resolve HEAD SHA for ${CURRENT_BRANCH}." >&2
  exit 1
fi

resolve_marker_paths "${PR_NUMBER}" "${HEAD_SHA}"
if ! begin_pr_bot_transaction; then
  exit 0
fi

echo "Confirmed PR #${PR_NUMBER} (${PR_URL}) for branch ${CURRENT_BRANCH}."
echo "Running pr-bot transaction..."

# Recursion guard: prevent PostRun hook from re-triggering pr-bot
# while inner CSA sessions spawned by this workflow complete.
export CSA_PR_BOT_GUARD=1

PLAN_RESULT_FILE="$(mktemp)"
set +e
csa plan run --sa-mode true patterns/pr-bot/workflow.toml | tee "${PLAN_RESULT_FILE}"
PLAN_RC=${PIPESTATUS[0]}
set -e

if grep -Eq '^[[:space:]]*ROUND_LIMIT_HALT:' "${PLAN_RESULT_FILE}"; then
  rm -f "${PLAN_RESULT_FILE}"
  touch "${PR_BOT_AWAITING_USER_MARKER}"
  rmdir "${PR_BOT_LOCK_DIR}" 2>/dev/null || true
  PR_BOT_LOCK_HELD=0
  echo "WARN: pr-bot reached REVIEW_ROUND limit and is awaiting manual follow-up; done marker was not written." >&2
  exit 0
fi

rm -f "${PLAN_RESULT_FILE}"

if [ "${PLAN_RC}" -eq 0 ]; then
  touch "${PR_BOT_DONE_MARKER}"
  rmdir "${PR_BOT_LOCK_DIR}" 2>/dev/null || true
  PR_BOT_LOCK_HELD=0
else
  echo "ERROR: pr-bot workflow failed for PR #${PR_NUMBER} at HEAD ${HEAD_SHA}." >&2
  exit 1
fi
