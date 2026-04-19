#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel)"
SCRIPT_PATH="${ROOT_DIR}/scripts/csa/cumulative-review-batch.sh"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

make_repo() {
  local repo_dir="$1"

  mkdir -p "${repo_dir}"
  git init "${repo_dir}" >/dev/null 2>&1
  git -C "${repo_dir}" config user.name "Test User"
  git -C "${repo_dir}" config user.email "test@example.com"
  git -C "${repo_dir}" checkout -b feat/review-batch >/dev/null 2>&1

  printf 'init\n' >"${repo_dir}/README.md"
  git -C "${repo_dir}" add README.md
  git -C "${repo_dir}" commit -m "init" >/dev/null 2>&1

  mkdir -p "${repo_dir}/scripts/csa"
  ln -s "${ROOT_DIR}/scripts/csa/session-wait-until-done.sh" \
    "${repo_dir}/scripts/csa/session-wait-until-done.sh"
}

add_commit() {
  local repo_dir="$1"
  local file_name="$2"
  local content="$3"
  local message="$4"

  printf '%s\n' "${content}" >"${repo_dir}/${file_name}"
  git -C "${repo_dir}" add "${file_name}"
  git -C "${repo_dir}" commit -m "${message}" >/dev/null 2>&1
}

make_csa_stub() {
  local stub_dir="$1"
  mkdir -p "${stub_dir}"

  cat >"${stub_dir}/csa" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

state_dir="${CSA_STUB_STATE_DIR:?}"
mkdir -p "${state_dir}"

cmd="${1:-}"
case "${cmd}" in
  config)
    shift
    subcmd="${1:-}"
    if [ "${subcmd}" != "show" ]; then
      echo "unexpected csa config subcommand: ${subcmd}" >&2
      exit 1
    fi
    printf '{"review":{"batch_commits":%s}}\n' "${CSA_STUB_BATCH_COMMITS:?}"
    ;;
  review)
    if [ "${CSA_STUB_REVIEW_FORBIDDEN:-0}" = "1" ]; then
      echo "review should not have been invoked" >&2
      exit 1
    fi
    count_file="${state_dir}/review-count"
    count=0
    if [ -f "${count_file}" ]; then
      count="$(cat "${count_file}")"
    fi
    count=$((count + 1))
    printf '%s' "${count}" >"${count_file}"

    session_id="${CSA_STUB_SESSION_ID:-01KTESTREVIEWBATCH0000000001}"
    project_root="$(pwd -P)"
    xdg_state_home="${XDG_STATE_HOME:-${HOME}/.local/state}"
    session_dir="${xdg_state_home}/cli-sub-agent/${project_root#/}/sessions/${session_id}"
    mkdir -p "${session_dir}/output"
    cat >"${session_dir}/output/review-verdict.json" <<JSON
{"severity_summary":{"critical":0,"high":0,"medium":0,"low":0,"info":0}}
JSON
    printf '%s\n' "${session_id}"
    ;;
  session)
    shift
    subcmd="${1:-}"
    if [ "${subcmd}" != "wait" ]; then
      echo "unexpected csa session subcommand: ${subcmd}" >&2
      exit 1
    fi
    echo "final_decision: CLEAN"
    ;;
  *)
    echo "unexpected csa command: ${cmd}" >&2
    exit 1
    ;;
esac
EOF
  chmod +x "${stub_dir}/csa"
}

assert_equals() {
  local expected="$1"
  local actual="$2"
  local message="$3"
  if [ "${expected}" != "${actual}" ]; then
    echo "${message}: expected '${expected}', got '${actual}'" >&2
    exit 1
  fi
}

run_skip_case() {
  local case_dir="${TMP_ROOT}/skip"
  local repo_dir="${case_dir}/repo"
  local stub_dir="${case_dir}/bin"
  local state_dir="${case_dir}/stub-state"
  local output_file="${case_dir}/output.log"
  local head_sha
  local state_file

  make_repo "${repo_dir}"
  add_commit "${repo_dir}" "file.txt" "one" "commit one"
  head_sha="$(git -C "${repo_dir}" rev-parse HEAD)"
  state_file="${repo_dir}/.csa/state/review/last-cumulative-feat__review-batch.txt"
  mkdir -p "$(dirname "${state_file}")"
  printf '%s\n' "${head_sha}" >"${state_file}"
  make_csa_stub "${stub_dir}"

  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" \
    XDG_STATE_HOME="${case_dir}/xdg-state" \
    CSA_STUB_STATE_DIR="${state_dir}" \
    CSA_STUB_BATCH_COMMITS="3" \
    CSA_STUB_REVIEW_FORBIDDEN="1" \
    bash "${SCRIPT_PATH}" --default-branch main -- csa review --range main...HEAD \
      >"${output_file}" 2>&1
  )

  grep -q "csa review: skip - batched" "${output_file}"
  if [ -f "${state_dir}/review-count" ]; then
    echo "review should not have been called in skip case" >&2
    exit 1
  fi
}

run_missing_state_runs_review_case() {
  local case_dir="${TMP_ROOT}/run"
  local repo_dir="${case_dir}/repo"
  local stub_dir="${case_dir}/bin"
  local state_dir="${case_dir}/stub-state"
  local output_file="${case_dir}/output.log"
  local state_file
  local head_sha

  make_repo "${repo_dir}"
  add_commit "${repo_dir}" "file.txt" "one" "commit one"
  head_sha="$(git -C "${repo_dir}" rev-parse HEAD)"
  state_file="${repo_dir}/.csa/state/review/last-cumulative-feat__review-batch.txt"
  make_csa_stub "${stub_dir}"

  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" \
    XDG_STATE_HOME="${case_dir}/xdg-state" \
    CSA_STUB_STATE_DIR="${state_dir}" \
    CSA_STUB_BATCH_COMMITS="3" \
    bash "${SCRIPT_PATH}" --default-branch main -- csa review --range main...HEAD \
      >"${output_file}" 2>&1
  )

  assert_equals "1" "$(cat "${state_dir}/review-count")" "run case review count"
  assert_equals "${head_sha}" "$(tr -d '\n' < "${state_file}")" "run case recorded head"
  grep -q "final_decision: CLEAN" "${output_file}"
}

run_override_case() {
  local case_dir="${TMP_ROOT}/override"
  local repo_dir="${case_dir}/repo"
  local stub_dir="${case_dir}/bin"
  local state_dir="${case_dir}/stub-state"
  local output_file="${case_dir}/output.log"
  local state_file
  local head_sha

  make_repo "${repo_dir}"
  add_commit "${repo_dir}" "file.txt" "one" "commit one"
  head_sha="$(git -C "${repo_dir}" rev-parse HEAD)"
  state_file="${repo_dir}/.csa/state/review/last-cumulative-feat__review-batch.txt"
  mkdir -p "$(dirname "${state_file}")"
  printf '%s\n' "${head_sha}" >"${state_file}"
  make_csa_stub "${stub_dir}"

  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" \
    XDG_STATE_HOME="${case_dir}/xdg-state" \
    CSA_STUB_STATE_DIR="${state_dir}" \
    CSA_STUB_BATCH_COMMITS="5" \
    CSA_REVIEW_NOW="1" \
    bash "${SCRIPT_PATH}" --default-branch main -- csa review --range main...HEAD \
      >"${output_file}" 2>&1
  )

  assert_equals "1" "$(cat "${state_dir}/review-count")" "override case review count"
  assert_equals "${head_sha}" "$(tr -d '\n' < "${state_file}")" "override case recorded head"
  grep -q "final_decision: CLEAN" "${output_file}"
}

run_skip_case
run_missing_state_runs_review_case
run_override_case

echo "cumulative-review-batch tests: ok"
