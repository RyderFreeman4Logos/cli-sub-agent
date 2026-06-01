#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel)"
SCRIPT_PATH="${ROOT_DIR}/scripts/csa/latest-pass-review-head.sh"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

make_repo() {
  local repo_dir="$1"

  mkdir -p "${repo_dir}"
  git init "${repo_dir}" >/dev/null 2>&1
  git -C "${repo_dir}" config user.name "Test User"
  git -C "${repo_dir}" config user.email "test@example.com"
  git -C "${repo_dir}" checkout -b feat/latest-pass >/dev/null 2>&1
  printf 'init\n' >"${repo_dir}/README.md"
  git -C "${repo_dir}" add README.md
  git -C "${repo_dir}" commit -m "init" >/dev/null 2>&1
}

make_csa_stub() {
  local stub_dir="$1"
  local session_id="$2"
  mkdir -p "${stub_dir}"

  cat >"${stub_dir}/csa" <<EOF
#!/usr/bin/env bash
set -euo pipefail

if [ "\${1:-}" = "session" ] && [ "\${2:-}" = "list" ]; then
  printf '%s\n' '[{"session_id":"${session_id}","last_accessed":"2026-04-01T00:00:00Z","task_context":{"task_type":"review"},"description":"review: latest pass"}]'
  exit 0
fi

echo "unexpected csa command: \$*" >&2
exit 1
EOF
  chmod +x "${stub_dir}/csa"
}

session_dir_for() {
  local repo_dir="$1"
  local state_home="$2"
  local session_id="$3"

  printf '%s/cli-sub-agent/%s/sessions/%s' \
    "${state_home}" \
    "${repo_dir#/}" \
    "${session_id}"
}

write_review_session() {
  local repo_dir="$1"
  local state_home="$2"
  local session_id="$3"
  local meta_json="$4"
  local verdict_json="$5"
  local session_dir

  session_dir="$(session_dir_for "${repo_dir}" "${state_home}" "${session_id}")"
  mkdir -p "${session_dir}/output"
  printf '%s\n' "${meta_json}" >"${session_dir}/review_meta.json"
  printf '%s\n' "${verdict_json}" >"${session_dir}/output/review-verdict.json"
}

assert_empty() {
  local actual="$1"
  local message="$2"

  if [ -n "${actual}" ]; then
    echo "${message}: expected empty output, got '${actual}'" >&2
    exit 1
  fi
}

run_case() {
  local case_name="$1"
  local meta_json="$2"
  local expected="$3"
  local case_dir="${TMP_ROOT}/${case_name}"
  local repo_dir="${case_dir}/repo"
  local state_home="${case_dir}/xdg-state"
  local stub_dir="${case_dir}/bin"
  local session_id="01KLATESTPASS00000000000001"
  local output

  make_repo "${repo_dir}"
  make_csa_stub "${stub_dir}" "${session_id}"
  write_review_session \
    "${repo_dir}" \
    "${state_home}" \
    "${session_id}" \
    "${meta_json}" \
    '{"decision":"pass","severity_counts":{"critical":0,"high":0,"medium":0,"low":0},"findings":[]}'

  output="$(
    cd "${repo_dir}"
    export PATH="${stub_dir}:${PATH}"
    export CSA_PROJECT_ROOT="${repo_dir}"
    export XDG_STATE_HOME="${state_home}"
    bash "${SCRIPT_PATH}" feat/latest-pass
  )"

  if [ -z "${expected}" ]; then
    assert_empty "${output}" "${case_name}"
  elif [ "${output}" != "${expected}" ]; then
    echo "${case_name}: expected '${expected}', got '${output}'" >&2
    exit 1
  fi
}

run_case \
  clean_non_fix \
  '{"session_id":"01KLATESTPASS00000000000001","head_sha":"cleanhead","decision":"pass","verdict":"CLEAN","tool":"codex","scope":"range:main...HEAD","exit_code":0,"fix_attempted":false,"fix_rounds":0,"timestamp":"2026-04-01T00:00:00Z"}' \
  cleanhead

run_case \
  pass_meta_exit_one \
  '{"session_id":"01KLATESTPASS00000000000001","head_sha":"badexit","decision":"pass","verdict":"CLEAN","tool":"codex","scope":"range:main...HEAD","exit_code":1,"fix_attempted":false,"fix_rounds":0,"timestamp":"2026-04-01T00:00:00Z"}' \
  ''

run_case \
  missing_fix_convergence \
  '{"session_id":"01KLATESTPASS00000000000001","head_sha":"missingfix","decision":"pass","verdict":"CLEAN","tool":"codex","scope":"range:main...HEAD","exit_code":0,"fix_attempted":true,"fix_rounds":3,"timestamp":"2026-04-01T00:00:00Z"}' \
  ''

run_case \
  false_fix_convergence \
  '{"session_id":"01KLATESTPASS00000000000001","head_sha":"falsefix","decision":"pass","verdict":"CLEAN","tool":"codex","scope":"range:main...HEAD","exit_code":0,"fix_attempted":true,"fix_rounds":3,"fix_convergence":{"quality_gate_passed":true,"fix_output_was_substantive":true,"post_consistency_decision":"fail","reached_genuine_clean_convergence":false,"terminal_reason":"post_consistency_non_pass"},"timestamp":"2026-04-01T00:00:00Z"}' \
  ''

run_case \
  true_fix_convergence \
  '{"session_id":"01KLATESTPASS00000000000001","head_sha":"truefix","decision":"pass","verdict":"CLEAN","tool":"codex","scope":"range:main...HEAD","exit_code":0,"fix_attempted":true,"fix_rounds":2,"fix_convergence":{"quality_gate_passed":true,"fix_output_was_substantive":true,"post_consistency_decision":"pass","reached_genuine_clean_convergence":true,"terminal_reason":"clean_convergence"},"timestamp":"2026-04-01T00:00:00Z"}' \
  truefix

echo "latest-pass-review-head tests: ok"
