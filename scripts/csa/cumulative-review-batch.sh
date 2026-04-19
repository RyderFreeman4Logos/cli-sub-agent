#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: cumulative-review-batch.sh --default-branch <branch> -- <csa review ...>

Runs a cumulative csa review unless batching says the intermediate review can
be skipped. Writes `.csa/state/review/last-cumulative-<branch>.txt` only when
the review session exits 0 and review-verdict.json reports zero HIGH/CRITICAL
findings.
EOF
  exit 2
}

if [ "$#" -lt 3 ]; then
  usage
fi

default_branch=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --default-branch)
      default_branch="${2:-}"
      shift 2
      ;;
    --)
      shift
      break
      ;;
    *)
      echo "ERROR: unexpected argument '$1'" >&2
      usage
      ;;
  esac
done

if [ -z "${default_branch}" ] || [ "$#" -eq 0 ]; then
  usage
fi

review_cmd=("$@")
project_root="$(pwd -P)"
current_branch="$(git branch --show-current)"

if [ -z "${current_branch}" ] || [ "${current_branch}" = "HEAD" ]; then
  echo "ERROR: Cannot determine current branch." >&2
  exit 1
fi

sanitize_branch() {
  printf '%s' "$1" | sed 's|/|__|g'
}

state_file_path() {
  local branch_name="$1"
  printf '.csa/state/review/last-cumulative-%s.txt' "$(sanitize_branch "${branch_name}")"
}

resolve_batch_commits() {
  local config_json
  config_json="$(csa config show --format json --cd "${project_root}")"
  jq -r '.review.batch_commits // 1' <<<"${config_json}"
}

resolve_session_dir() {
  local session_id="$1"
  local state_home="${XDG_STATE_HOME:-${HOME}/.local/state}"
  printf '%s/cli-sub-agent/%s/sessions/%s' "${state_home}" "${project_root#/}" "${session_id}"
}

should_record_passed_head() {
  local session_id="$1"
  local session_dir verdict_path
  session_dir="$(resolve_session_dir "${session_id}")"
  verdict_path="${session_dir}/output/review-verdict.json"

  if [ ! -f "${verdict_path}" ]; then
    echo "WARN: review-verdict artifact missing for session ${session_id}; not updating batch state." >&2
    return 1
  fi

  # Do not gate on .decision here: a known review-meta parsing bug can emit
  # decision=fail even when severity_counts correctly reports no blocking findings.
  jq -e '
    (.severity_counts.critical // 0) == 0
    and (.severity_counts.high // 0) == 0
  ' "${verdict_path}" >/dev/null
}

batch_commits="$(resolve_batch_commits)"
if ! [[ "${batch_commits}" =~ ^[0-9]+$ ]]; then
  echo "ERROR: review.batch_commits must be an integer, got '${batch_commits}'." >&2
  exit 1
fi

last_sha_file="$(state_file_path "${current_branch}")"
if [ "${CSA_REVIEW_NOW:-0}" != "1" ] && [ "${batch_commits}" -ge 2 ] && [ -f "${last_sha_file}" ]; then
  last_sha="$(tr -d '\n' < "${last_sha_file}")"
  if [ -n "${last_sha}" ] && git merge-base --is-ancestor "${last_sha}" HEAD 2>/dev/null; then
    new_commits="$(git rev-list --count "${last_sha}..HEAD")"
    if [ "${new_commits}" -lt "${batch_commits}" ]; then
      echo "csa review: skip - batched (${new_commits}/${batch_commits} commits since last passed cumulative review ${last_sha:0:8})"
      exit 0
    fi
  fi
fi

review_output_file="$(mktemp)"
trap 'rm -f "${review_output_file}"' EXIT

set +e
sid="$("${review_cmd[@]}")"
bash scripts/csa/session-wait-until-done.sh "${sid}" 2>&1 | tee "${review_output_file}"
review_status=${PIPESTATUS[0]}
set -e

if [ "${review_status}" -ne 0 ]; then
  exit "${review_status}"
fi

if should_record_passed_head "${sid}"; then
  mkdir -p "$(dirname "${last_sha_file}")"
  git rev-parse HEAD > "${last_sha_file}"
fi
