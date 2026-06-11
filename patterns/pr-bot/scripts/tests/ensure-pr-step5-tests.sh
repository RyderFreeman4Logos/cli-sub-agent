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
  local expected_title="${11:-Fix 1171}"
  local pr_title_mode="${12:-set}"
  local git_head_subject="${13-Fix 1171}"
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
    export PATH="${bin_dir}:${PATH}"
    export GIT_STUB_STATE_DIR="${case_dir}"
    export GIT_STUB_BRANCH_PUSHED="${branch_pushed}"
    export GIT_STUB_SOURCE_OWNER="test-owner"
    export GIT_STUB_HEAD_SUBJECT="${git_head_subject}"
    export GH_STUB_STATE_DIR="${case_dir}"
    export GH_STUB_SCENARIO="${scenario}"
    export GH_STUB_EXPECTED_LIST_HEAD="${expected_list_head}"
    export GH_STUB_EXPECTED_CREATE_HEAD="${expected_create_head}"
    export GH_STUB_EXPECTED_BASE="main"
    export GH_STUB_EXPECTED_TITLE="${expected_title}"
    export REVIEW_COMPLETED="true"
    export REMOTE_NAME="origin"
    export WORKFLOW_BRANCH="${workflow_branch}"
    export REPO_SLUG="test-owner/test-repo"
    export DEFAULT_BRANCH="main"
    export PR_BODY="Body"
    if [ "${pr_title_mode}" = "unset" ]; then
      unset PR_TITLE
    else
      export PR_TITLE="${expected_title}"
    fi
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
  if [ "${scenario}" = "merged" ]; then
    grep -q "CSA_VAR:MERGE_COMPLETED=true" "${stdout_file}"
  fi
  if [ -n "${expected_error}" ]; then
    grep -q "${expected_error}" "${stderr_file}"
  fi
}

run_case "branch-unpushed-create" "false" "create-success" "0" "101" "1"
run_case "branch-pushed-create" "true" "create-success" "0" "101" "1"
run_case "lookup-hits-reuse" "true" "preexisting" "0" "202" "0"
run_case "lookup-hits-merged-noop" "true" "merged" "0" "909" "0"
run_case "cross-owner-create" "true" "cross-owner" "0" "101" "1"
run_case "unset-title-derives-head" "true" "create-success" "0" "101" "1" "PR_TITLE unset; using derived title: fix(pr-bot): derive title" "fix/1171" "fix/1171" "test-owner:fix/1171" "fix(pr-bot): derive title" "unset" "fix(pr-bot): derive title"
run_case "unset-title-falls-back-to-branch" "true" "create-success" "0" "101" "1" "PR_TITLE unset; using derived title: Topic custom title" "topic/custom_title" "topic/custom_title" "test-owner:topic/custom_title" "Topic custom title" "unset" ""
run_case "create-already-exists-reresolve" "true" "missed-already-exists" "0" "303" "1" "PR already exists for test-owner:fix/1171; re-resolving"
run_case "stale-list-create-race-recovery" "true" "stale-already-exists" "0" "808" "1" "PR already exists for test-owner:fix/1171; re-resolving"
run_case "ambiguous-fail-closed" "true" "ambiguous" "1" "" "0" "Multiple PRs found for test-owner:fix/1171"
run_case "closed-pr-fail-clear" "true" "closed" "1" "" "0" "is closed but not merged"
run_case "quoted-branch-reuse" "true" "quoted-branch" "0" "707" "0" "" 'feat/has"quote'

echo "ensure-pr Step 5 tests: PASS"
