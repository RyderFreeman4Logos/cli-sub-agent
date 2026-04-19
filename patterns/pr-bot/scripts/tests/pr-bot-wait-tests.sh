#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel)"
SCRIPT_PATH="${ROOT_DIR}/patterns/pr-bot/scripts/pr-bot-wait.sh"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

make_repo() {
  local repo_dir="$1"
  local commit_date="${2:-2026-04-19T00:00:00Z}"
  mkdir -p "${repo_dir}"
  git init "${repo_dir}" >/dev/null 2>&1
  git -C "${repo_dir}" config user.name "Test User"
  git -C "${repo_dir}" config user.email "test@example.com"
  git -C "${repo_dir}" remote add origin "git@github.com:test-owner/test-repo.git"
  printf 'test\n' >"${repo_dir}/README.md"
  git -C "${repo_dir}" add README.md
  GIT_AUTHOR_DATE="${commit_date}" GIT_COMMITTER_DATE="${commit_date}" \
    git -C "${repo_dir}" commit -m "init" >/dev/null 2>&1
}

make_gh_stub() {
  local stub_dir="$1"
  mkdir -p "${stub_dir}"
  cat >"${stub_dir}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

state_dir="${GH_STUB_STATE_DIR:?}"
scenario="${GH_STUB_SCENARIO:?}"
push_sha="${GH_STUB_PUSH_SHA:-PUSH_SHA}"

if [ "${1:-}" = "api" ]; then
  shift
  while [ "$#" -gt 0 ] && [ "${1}" = "--paginate" -o "${1}" = "--slurp" ]; do
    shift
  done
  endpoint="${1:-}"
  case "${endpoint}" in
    repos/test-owner/test-repo/pulls/*/reviews?per_page=100)
      review_count_file="${state_dir}/review-count"
      review_count=0
      if [ -f "${review_count_file}" ]; then
        review_count="$(cat "${review_count_file}")"
      fi
      review_count=$((review_count + 1))
      printf '%s' "${review_count}" >"${review_count_file}"
      case "${scenario}" in
        replied)
          cat <<'JSON'
[[{"id":101,"state":"COMMENTED","submitted_at":"2026-04-19T10:01:00Z","commit_id":"__PUSH_SHA__","user":{"login":"test-bot[bot]","type":"Bot"}}]]
JSON
          ;;
        replied-null-commit)
          cat <<'JSON'
[[{"id":102,"state":"COMMENTED","submitted_at":"2026-04-19T10:02:00Z","commit_id":null,"user":{"login":"test-bot[bot]","type":"Bot"}}]]
JSON
          ;;
        quiet-wait-gap)
          cat <<'JSON'
[[{"id":103,"state":"COMMENTED","submitted_at":"2026-04-19T00:05:00Z","commit_id":null,"user":{"login":"test-bot[bot]","type":"Bot"}}]]
JSON
          ;;
        quota|timeout)
          echo '[[]]'
          ;;
        *)
          echo "unknown scenario: ${scenario}" >&2
          exit 1
          ;;
      esac
      ;;
    repos/test-owner/test-repo/issues/*/comments?per_page=100)
      comment_count_file="${state_dir}/comment-count"
      comment_count=0
      if [ -f "${comment_count_file}" ]; then
        comment_count="$(cat "${comment_count_file}")"
      fi
      comment_count=$((comment_count + 1))
      printf '%s' "${comment_count}" >"${comment_count_file}"
      case "${scenario}" in
        replied|quiet-wait-gap)
          echo '[[]]'
          ;;
        quota)
          if [ "${comment_count}" -lt 4 ]; then
            echo '[[]]'
          else
            cat <<'JSON'
[[{"id":202,"created_at":"2026-04-19T10:04:00Z","body":"Daily quota limit reached for today.","user":{"login":"test-bot[bot]","type":"Bot"}}]]
JSON
          fi
          ;;
        timeout)
          echo '[[]]'
          ;;
        *)
          echo "unknown scenario: ${scenario}" >&2
          exit 1
          ;;
      esac
      ;;
    *)
      echo "unexpected gh api endpoint: ${endpoint}" >&2
      exit 1
      ;;
  esac
  exit 0
fi

echo "unexpected gh command: $*" >&2
exit 1
EOF
  sed -i "s/__PUSH_SHA__/${GH_STUB_PUSH_SHA:-PUSH_SHA}/g" "${stub_dir}/gh"
  chmod +x "${stub_dir}/gh"
}

assert_json_value() {
  local file="$1"
  local jq_expr="$2"
  local expected="$3"
  local actual
  actual="$(jq -r "${jq_expr}" "${file}")"
  if [ "${actual}" != "${expected}" ]; then
    echo "assertion failed: ${jq_expr} expected '${expected}', got '${actual}'" >&2
    exit 1
  fi
}

run_wait_wrapper_loop() {
  local output_file="$1"
  local poll_pid="$2"
  local wait_long_poll_secs="$3"
  local cloud_bot_poll_max_seconds="$4"
  local pr_number="$5"
  local wait_result_grace_secs="${6:-1}"
  local wait_started_at
  local wait_elapsed_secs

  wait_started_at="$(date +%s)"
  wait_elapsed_secs=0

  while [ "${wait_elapsed_secs}" -lt "${cloud_bot_poll_max_seconds}" ]; do
    if [ -f "${output_file}" ]; then
      break
    fi
    if ! kill -0 "${poll_pid}" 2>/dev/null; then
      sleep "${wait_result_grace_secs}"
      break
    fi

    local remaining=$((cloud_bot_poll_max_seconds - wait_elapsed_secs))
    local step="${wait_long_poll_secs}"
    if [ "${step}" -gt "${remaining}" ]; then
      step="${remaining}"
    fi

    sleep "${step}" &
    local wait_sleep_pid=$!
    wait -n "${poll_pid}" "${wait_sleep_pid}" 2>/dev/null || true
    if kill -0 "${wait_sleep_pid}" 2>/dev/null; then
      kill "${wait_sleep_pid}" 2>/dev/null || true
      wait "${wait_sleep_pid}" 2>/dev/null || true
    fi
    wait_elapsed_secs=$(( $(date +%s) - wait_started_at ))
  done

  wait_elapsed_secs=$(( $(date +%s) - wait_started_at ))

  if [ ! -f "${output_file}" ]; then
    if kill -0 "${poll_pid}" 2>/dev/null; then
      kill "${poll_pid}" 2>/dev/null || true
      wait "${poll_pid}" 2>/dev/null || true
    fi
    printf '{"status":"timeout","pr":%s,"elapsed_seconds":%s}\n' \
      "${pr_number}" "${wait_elapsed_secs}" > "${output_file}.tmp"
    mv "${output_file}.tmp" "${output_file}"
  fi
}

SPAWNED_WAIT_WRAPPER_PID=""

spawn_wait_wrapper_helper() {
  local output_file="$1"
  local delay_seconds="$2"
  local status="$3"

  (
    sleep "${delay_seconds}"
    printf '{"status":"%s"}\n' "${status}" > "${output_file}.tmp"
    mv "${output_file}.tmp" "${output_file}"
  ) &
  SPAWNED_WAIT_WRAPPER_PID="$!"
}

assert_elapsed_between() {
  local actual="$1"
  local min_expected="$2"
  local max_expected="$3"
  local description="$4"

  if [ "${actual}" -lt "${min_expected}" ] || [ "${actual}" -gt "${max_expected}" ]; then
    echo "${description}: expected elapsed in [${min_expected}, ${max_expected}], got ${actual}" >&2
    exit 1
  fi
}

run_replied_test() {
  local case_dir="${TMP_ROOT}/replied"
  local repo_dir="${case_dir}/repo"
  local stub_dir="${case_dir}/bin"
  local output_file="${case_dir}/result.json"
  local push_sha

  make_repo "${repo_dir}"
  push_sha="$(git -C "${repo_dir}" rev-parse HEAD)"
  GH_STUB_PUSH_SHA="${push_sha}" make_gh_stub "${stub_dir}"

  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" \
    GH_STUB_STATE_DIR="${case_dir}" \
    GH_STUB_SCENARIO="replied" \
    GH_STUB_PUSH_SHA="${push_sha}" \
    CSA_PR_BOT_NAME="test-bot" \
    "${SCRIPT_PATH}" 42 \
      --timeout 3 \
      --interval 1 \
      --bot-login "test-bot[bot]" \
      --push-sha "${push_sha}" \
      --output "${output_file}"
  )

  assert_json_value "${output_file}" '.status' 'replied'
  assert_json_value "${output_file}" '.review.id' '101'
  assert_json_value "${output_file}" '.review.commit_id' "${push_sha}"
}

run_quota_test() {
  local case_dir="${TMP_ROOT}/quota"
  local repo_dir="${case_dir}/repo"
  local stub_dir="${case_dir}/bin"
  local output_file="${case_dir}/result.json"
  local quota_cache="${case_dir}/quota.toml"
  local push_sha

  make_repo "${repo_dir}"
  push_sha="$(git -C "${repo_dir}" rev-parse HEAD)"
  GH_STUB_PUSH_SHA="${push_sha}" make_gh_stub "${stub_dir}"

  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" \
    GH_STUB_STATE_DIR="${case_dir}" \
    GH_STUB_SCENARIO="quota" \
    GH_STUB_PUSH_SHA="${push_sha}" \
    CSA_PR_BOT_NAME="test-bot" \
    "${SCRIPT_PATH}" 77 \
      --timeout 4 \
      --interval 1 \
      --bot-login "test-bot[bot]" \
      --push-sha "${push_sha}" \
      --quota-cache "${quota_cache}" \
      --output "${output_file}"
  )

  assert_json_value "${output_file}" '.status' 'quota_exhausted'
  assert_json_value "${output_file}" '.pr' '77'
  grep -q '\[cloud_bot_quota.test-bot\]' "${quota_cache}"
  grep -q 'last_pr_seen = 77' "${quota_cache}"
}

run_null_commit_replied_test() {
  local case_dir="${TMP_ROOT}/replied-null-commit"
  local repo_dir="${case_dir}/repo"
  local stub_dir="${case_dir}/bin"
  local output_file="${case_dir}/result.json"
  local push_sha

  make_repo "${repo_dir}"
  push_sha="$(git -C "${repo_dir}" rev-parse HEAD)"
  GH_STUB_PUSH_SHA="${push_sha}" make_gh_stub "${stub_dir}"

  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" \
    GH_STUB_STATE_DIR="${case_dir}" \
    GH_STUB_SCENARIO="replied-null-commit" \
    GH_STUB_PUSH_SHA="${push_sha}" \
    CSA_PR_BOT_NAME="test-bot" \
    "${SCRIPT_PATH}" 52 \
      --timeout 3 \
      --interval 1 \
      --bot-login "test-bot[bot]" \
      --push-sha "${push_sha}" \
      --window-start "2026-04-19T10:00:00Z" \
      --output "${output_file}"
  )

  assert_json_value "${output_file}" '.status' 'replied'
  assert_json_value "${output_file}" '.review.id' '102'
  assert_json_value "${output_file}" '.review.commit_id' 'null'
}

run_timeout_test() {
  local case_dir="${TMP_ROOT}/timeout"
  local repo_dir="${case_dir}/repo"
  local stub_dir="${case_dir}/bin"
  local output_file="${case_dir}/result.json"
  local push_sha

  make_repo "${repo_dir}"
  push_sha="$(git -C "${repo_dir}" rev-parse HEAD)"
  GH_STUB_PUSH_SHA="${push_sha}" make_gh_stub "${stub_dir}"

  set +e
  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" \
    GH_STUB_STATE_DIR="${case_dir}" \
    GH_STUB_SCENARIO="timeout" \
    GH_STUB_PUSH_SHA="${push_sha}" \
    CSA_PR_BOT_NAME="test-bot" \
    "${SCRIPT_PATH}" 99 \
      --timeout 3 \
      --interval 1 \
      --bot-login "test-bot[bot]" \
      --push-sha "${push_sha}" \
      --output "${output_file}"
  )
  rc=$?
  set -e

  if [ "${rc}" -ne 1 ]; then
    echo "timeout scenario expected exit 1, got ${rc}" >&2
    exit 1
  fi

  assert_json_value "${output_file}" '.status' 'timeout'
  assert_json_value "${output_file}" '.elapsed_seconds' '3'
}

run_quiet_wait_window_regression_test() {
  local case_dir="${TMP_ROOT}/quiet-wait-gap"
  local repo_dir="${case_dir}/repo"
  local stub_dir="${case_dir}/bin"
  local output_with_window="${case_dir}/result-with-window.json"
  local output_without_window="${case_dir}/result-without-window.json"
  local push_sha

  make_repo "${repo_dir}" "2026-04-19T00:00:00Z"
  push_sha="$(git -C "${repo_dir}" rev-parse HEAD)"
  GH_STUB_PUSH_SHA="${push_sha}" make_gh_stub "${stub_dir}"

  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" \
    GH_STUB_STATE_DIR="${case_dir}" \
    GH_STUB_SCENARIO="quiet-wait-gap" \
    GH_STUB_PUSH_SHA="${push_sha}" \
    CSA_PR_BOT_NAME="test-bot" \
    "${SCRIPT_PATH}" 64 \
      --timeout 3 \
      --interval 1 \
      --bot-login "test-bot[bot]" \
      --push-sha "${push_sha}" \
      --window-start "2026-04-19T00:00:00Z" \
      --output "${output_with_window}"
  )

  assert_json_value "${output_with_window}" '.status' 'replied'
  assert_json_value "${output_with_window}" '.review.id' '103'
  assert_json_value "${output_with_window}" '.review.commit_id' 'null'

  set +e
  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" \
    GH_STUB_STATE_DIR="${case_dir}" \
    GH_STUB_SCENARIO="quiet-wait-gap" \
    GH_STUB_PUSH_SHA="${push_sha}" \
    CSA_PR_BOT_NAME="test-bot" \
    "${SCRIPT_PATH}" 64 \
      --timeout 3 \
      --interval 1 \
      --bot-login "test-bot[bot]" \
      --push-sha "${push_sha}" \
      --output "${output_without_window}"
  )
  rc=$?
  set -e

  if [ "${rc}" -ne 1 ]; then
    echo "quiet-wait regression scenario without window-start expected exit 1, got ${rc}" >&2
    exit 1
  fi

  assert_json_value "${output_without_window}" '.status' 'timeout'
}

run_wait_wrapper_short_helper_timeout_test() {
  local case_dir="${TMP_ROOT}/wait-wrapper-short-helper"
  local output_file="${case_dir}/result.json"
  local started_at
  local elapsed_seconds

  mkdir -p "${case_dir}"
  spawn_wait_wrapper_helper "${output_file}" 3 replied
  started_at="$(date +%s)"
  run_wait_wrapper_loop "${output_file}" "${SPAWNED_WAIT_WRAPPER_PID}" 240 30 42
  elapsed_seconds=$(( $(date +%s) - started_at ))

  assert_elapsed_between "${elapsed_seconds}" 2 5 "short helper timeout wrapper regression"
  assert_json_value "${output_file}" '.status' 'replied'
}

run_wait_wrapper_long_helper_timeout_test() {
  local case_dir="${TMP_ROOT}/wait-wrapper-long-helper"
  local output_file="${case_dir}/result.json"
  local started_at
  local elapsed_seconds

  mkdir -p "${case_dir}"
  spawn_wait_wrapper_helper "${output_file}" 15 timeout
  started_at="$(date +%s)"
  run_wait_wrapper_loop "${output_file}" "${SPAWNED_WAIT_WRAPPER_PID}" 10 15 77
  elapsed_seconds=$(( $(date +%s) - started_at ))

  assert_elapsed_between "${elapsed_seconds}" 14 20 "long helper timeout wrapper regression"
  assert_json_value "${output_file}" '.status' 'timeout'
}

run_replied_test
run_quota_test
run_null_commit_replied_test
run_timeout_test
run_quiet_wait_window_regression_test
run_wait_wrapper_short_helper_timeout_test
run_wait_wrapper_long_helper_timeout_test

echo "pr-bot-wait tests: PASS"
