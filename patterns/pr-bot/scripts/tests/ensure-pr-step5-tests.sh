#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel)"
WORKFLOW_PATH="${ROOT_DIR}/patterns/pr-bot/workflow.toml"
STUB_DIR="${ROOT_DIR}/patterns/pr-bot/scripts/tests/_stubs"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

if ! command -v jq >/dev/null 2>&1; then
  echo "ensure-pr Step 5 tests: SKIP (jq is required for client-side PR lookup filtering)" >&2
  exit 0
fi

extract_step5_script() {
  local output_path="$1"
  awk '
    $0 == "id = 5" { in_step = 1 }
    in_step && $0 == "```bash" { in_code = 1; next }
    in_code && $0 ~ /^```/ { exit }
    in_code { print }
  ' "${WORKFLOW_PATH}" >"${output_path}"
  chmod +x "${output_path}"
}

assert_file_value() {
  local file="$1"
  local expected="$2"
  local actual="0"
  if [ -f "${file}" ]; then
    actual="$(<"${file}")"
  fi
  if [ "${actual}" != "${expected}" ]; then
    echo "expected ${file} to contain ${expected}, got ${actual}" >&2
    exit 1
  fi
}

run_case() {
  local case_name="$1"
  local branch_pushed="$2"
  local scenario="$3"
  local expected_rc="$4"
  local expected_pr="$5"
  local expected_creates="$6"
  local expected_error="${7:-}"
  local workflow_branch="${8:-fix/1171}"
  local expected_list_head="${9:-${workflow_branch}}"
  local expected_create_head="${10:-test-owner:${workflow_branch}}"
  local case_dir="${TMP_ROOT}/${case_name}"
  local bin_dir="${case_dir}/bin"
  local stdout_file="${case_dir}/stdout.txt"
  local stderr_file="${case_dir}/stderr.txt"
  local script_path="${case_dir}/step5.sh"
  local rc

  mkdir -p "${bin_dir}"
  ln -s "${STUB_DIR}/git-stub.sh" "${bin_dir}/git"
  ln -s "${STUB_DIR}/gh-stub.sh" "${bin_dir}/gh"
  extract_step5_script "${script_path}"

  set +e
  (
    PATH="${bin_dir}:${PATH}" \
    GIT_STUB_STATE_DIR="${case_dir}" \
    GIT_STUB_BRANCH_PUSHED="${branch_pushed}" \
    GIT_STUB_SOURCE_OWNER="test-owner" \
    GH_STUB_STATE_DIR="${case_dir}" \
    GH_STUB_SCENARIO="${scenario}" \
    GH_STUB_EXPECTED_LIST_HEAD="${expected_list_head}" \
    GH_STUB_EXPECTED_CREATE_HEAD="${expected_create_head}" \
    GH_STUB_EXPECTED_BASE="main" \
    REVIEW_COMPLETED="true" \
    REMOTE_NAME="origin" \
    WORKFLOW_BRANCH="${workflow_branch}" \
    REPO_SLUG="test-owner/test-repo" \
    DEFAULT_BRANCH="main" \
    PR_TITLE="Fix 1171" \
    PR_BODY="Body" \
    "${script_path}" >"${stdout_file}" 2>"${stderr_file}"
  )
  rc=$?
  set -e

  if [ "${rc}" != "${expected_rc}" ]; then
    echo "${case_name}: expected rc ${expected_rc}, got ${rc}" >&2
    echo "--- stdout ---" >&2
    sed -n '1,120p' "${stdout_file}" >&2
    echo "--- stderr ---" >&2
    sed -n '1,120p' "${stderr_file}" >&2
    exit 1
  fi

  assert_file_value "${case_dir}/push-count" "1"
  assert_file_value "${case_dir}/pr-create-count" "${expected_creates}"

  if [ "${expected_rc}" = "0" ]; then
    grep -q "CSA_VAR:PR_NUM=${expected_pr}" "${stdout_file}"
  fi
  if [ -n "${expected_error}" ]; then
    grep -q "${expected_error}" "${stderr_file}"
  fi
}

run_case "branch-unpushed-create" "false" "create-success" "0" "101" "1"
run_case "branch-pushed-create" "true" "create-success" "0" "101" "1"
run_case "lookup-hits-reuse" "true" "preexisting" "0" "202" "0"
run_case "cross-owner-create" "true" "cross-owner" "0" "101" "1"
run_case "create-already-exists-reresolve" "true" "missed-already-exists" "0" "303" "1" "PR already exists for test-owner:fix/1171; re-resolving"
run_case "stale-list-create-race-recovery" "true" "stale-already-exists" "0" "808" "1" "PR already exists for test-owner:fix/1171; re-resolving"
run_case "ambiguous-fail-closed" "true" "ambiguous" "1" "" "0" "Multiple open PRs found for test-owner:fix/1171"
run_case "quoted-branch-reuse" "true" "quoted-branch" "0" "707" "0" "" 'feat/has"quote'

echo "ensure-pr Step 5 tests: PASS"
