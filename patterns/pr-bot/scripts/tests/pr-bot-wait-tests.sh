#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel)"
SCRIPT_PATH="${ROOT_DIR}/patterns/pr-bot/scripts/pr-bot-wait.sh"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

make_repo() {
  local repo_dir="$1"
  mkdir -p "${repo_dir}"
  git init "${repo_dir}" >/dev/null 2>&1
  git -C "${repo_dir}" config user.name "Test User"
  git -C "${repo_dir}" config user.email "test@example.com"
  git -C "${repo_dir}" remote add origin "git@github.com:test-owner/test-repo.git"
  printf 'test\n' >"${repo_dir}/README.md"
  git -C "${repo_dir}" add README.md
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
        replied)
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

run_replied_test
run_quota_test
run_null_commit_replied_test
run_timeout_test

echo "pr-bot-wait tests: PASS"
