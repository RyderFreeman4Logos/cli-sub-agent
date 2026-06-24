#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel)"
SCRIPT_PATH="${ROOT_DIR}/patterns/pr-bot/scripts/resolve-push-remote.sh"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

make_repo() {
  local repo_dir="$1"
  mkdir -p "${repo_dir}"
  git init "${repo_dir}" >/dev/null 2>&1
  git -C "${repo_dir}" config user.name "Test User"
  git -C "${repo_dir}" config user.email "test@example.com"
  printf 'test\n' >"${repo_dir}/README.md"
  git -C "${repo_dir}" add README.md
  git -C "${repo_dir}" commit -m "init" >/dev/null 2>&1
  git -C "${repo_dir}" branch -M main
}

make_gh_stub() {
  local stub_dir="$1"
  local login="$2"
  mkdir -p "${stub_dir}"
  cat >"${stub_dir}/gh" <<EOF
#!/usr/bin/env bash
set -euo pipefail

if [ "\${1:-}" = "api" ] && [ "\${2:-}" = "user" ] && [ "\${3:-}" = "--jq" ] && [ "\${4:-}" = ".login" ]; then
  printf '%s\n' "${login}"
  exit 0
fi

echo "unexpected gh invocation: \$*" >&2
exit 1
EOF
  chmod +x "${stub_dir}/gh"
}

run_ambiguous_origin_fail_closed_test() {
  local case_dir="${TMP_ROOT}/ambiguous-origin"
  local repo_dir="${case_dir}/repo"
  local stub_dir="${case_dir}/bin"
  local stderr_file="${case_dir}/stderr.txt"

  make_repo "${repo_dir}"
  git -C "${repo_dir}" checkout -b fix/917-fork-convention >/dev/null 2>&1
  git -C "${repo_dir}" remote add origin "git@github.com:canonical-org/cli-sub-agent.git"
  git -C "${repo_dir}" remote add fork "git@github.com:test-user/cli-sub-agent.git"
  git -C "${repo_dir}" config "branch.fix/917-fork-convention.remote" "fork"
  make_gh_stub "${stub_dir}" "test-user"

  set +e
  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" "${SCRIPT_PATH}" "fix/917-fork-convention" >"${case_dir}/stdout.txt" 2>"${stderr_file}"
  )
  rc=$?
  set -e

  if [ "${rc}" -ne 2 ]; then
    echo "expected ambiguous origin scenario to exit 2, got ${rc}" >&2
    exit 1
  fi

  grep -q "pr-bot detected an ambiguous fork convention" "${stderr_file}"
  grep -q "git config branch.fix/917-fork-convention.pushRemote <your-fork-remote-name>" "${stderr_file}"
}

run_explicit_push_remote_wins_test() {
  local case_dir="${TMP_ROOT}/push-remote"
  local repo_dir="${case_dir}/repo"
  local stub_dir="${case_dir}/bin"
  local resolved_remote

  make_repo "${repo_dir}"
  git -C "${repo_dir}" checkout -b fix/917-explicit-remote >/dev/null 2>&1
  git -C "${repo_dir}" remote add origin "git@github.com:canonical-org/cli-sub-agent.git"
  git -C "${repo_dir}" remote add fork "git@github.com:test-user/cli-sub-agent.git"
  git -C "${repo_dir}" config "branch.fix/917-explicit-remote.pushRemote" "fork"
  make_gh_stub "${stub_dir}" "test-user"

  resolved_remote="$(
    cd "${repo_dir}" &&
      PATH="${stub_dir}:${PATH}" "${SCRIPT_PATH}" "fix/917-explicit-remote"
  )"

  if [ "${resolved_remote}" != "fork" ]; then
    echo "expected explicit pushRemote to resolve to fork, got ${resolved_remote}" >&2
    exit 1
  fi
}

run_branch_push_remote_wins_over_invalid_push_default_test() {
  local case_dir="${TMP_ROOT}/branch-push-over-invalid-default"
  local repo_dir="${case_dir}/repo"
  local resolved_remote

  make_repo "${repo_dir}"
  git -C "${repo_dir}" checkout -b fix/2406-branch-push-wins >/dev/null 2>&1
  git -C "${repo_dir}" remote add origin "git@github.com:canonical-org/cli-sub-agent.git"
  git -C "${repo_dir}" remote add fork "git@github.com:test-user/cli-sub-agent.git"
  git -C "${repo_dir}" config "branch.fix/2406-branch-push-wins.pushRemote" "fork"
  git -C "${repo_dir}" config remote.pushDefault "missing-fork"

  resolved_remote="$(
    cd "${repo_dir}" &&
      "${SCRIPT_PATH}" "fix/2406-branch-push-wins"
  )"

  if [ "${resolved_remote}" != "fork" ]; then
    echo "expected branch pushRemote to win over invalid remote.pushDefault, got ${resolved_remote}" >&2
    exit 1
  fi
}

run_invalid_branch_push_remote_fail_closed_test() {
  local case_dir="${TMP_ROOT}/invalid-branch-push-remote"
  local repo_dir="${case_dir}/repo"
  local stderr_file="${case_dir}/stderr.txt"
  local stdout_file="${case_dir}/stdout.txt"

  make_repo "${repo_dir}"
  git -C "${repo_dir}" checkout -b fix/2406-invalid-branch-push >/dev/null 2>&1
  git -C "${repo_dir}" remote add origin "git@github.com:canonical-org/cli-sub-agent.git"
  git -C "${repo_dir}" config "branch.fix/2406-invalid-branch-push.pushRemote" "missing-fork"

  set +e
  (
    cd "${repo_dir}"
    "${SCRIPT_PATH}" "fix/2406-invalid-branch-push" >"${stdout_file}" 2>"${stderr_file}"
  )
  rc=$?
  set -e

  if [ "${rc}" -ne 1 ]; then
    echo "expected invalid branch pushRemote to exit 1, got ${rc}" >&2
    exit 1
  fi
  if grep -q "^origin$" "${stdout_file}"; then
    echo "invalid branch pushRemote must not fall back to origin" >&2
    exit 1
  fi

  grep -q "invalid explicit pr-bot push remote" "${stderr_file}"
  grep -q "branch.fix/2406-invalid-branch-push.pushRemote: missing-fork" "${stderr_file}"
  grep -q "git config branch.fix/2406-invalid-branch-push.pushRemote <remote-name>" "${stderr_file}"
  grep -q "git config --unset branch.fix/2406-invalid-branch-push.pushRemote" "${stderr_file}"
}

run_invalid_remote_push_default_fail_closed_test() {
  local case_dir="${TMP_ROOT}/invalid-push-default"
  local repo_dir="${case_dir}/repo"
  local stderr_file="${case_dir}/stderr.txt"
  local stdout_file="${case_dir}/stdout.txt"

  make_repo "${repo_dir}"
  git -C "${repo_dir}" checkout -b fix/2406-invalid-push-default >/dev/null 2>&1
  git -C "${repo_dir}" remote add origin "git@github.com:canonical-org/cli-sub-agent.git"
  git -C "${repo_dir}" config remote.pushDefault "missing-fork"

  set +e
  (
    cd "${repo_dir}"
    "${SCRIPT_PATH}" "fix/2406-invalid-push-default" >"${stdout_file}" 2>"${stderr_file}"
  )
  rc=$?
  set -e

  if [ "${rc}" -ne 1 ]; then
    echo "expected invalid remote.pushDefault to exit 1, got ${rc}" >&2
    exit 1
  fi
  if grep -q "^origin$" "${stdout_file}"; then
    echo "invalid remote.pushDefault must not fall back to origin" >&2
    exit 1
  fi

  grep -q "invalid explicit pr-bot push remote" "${stderr_file}"
  grep -q "remote.pushDefault: missing-fork" "${stderr_file}"
  grep -q "git config remote.pushDefault <remote-name>" "${stderr_file}"
  grep -q "git config --unset remote.pushDefault" "${stderr_file}"
}

run_local_only_branch_origin_fallback_test() {
  local case_dir="${TMP_ROOT}/local-only-origin"
  local repo_dir="${case_dir}/repo"
  local stub_dir="${case_dir}/bin"
  local resolved_remote

  make_repo "${repo_dir}"
  git -C "${repo_dir}" checkout -b fix/2406-local-only >/dev/null 2>&1
  git -C "${repo_dir}" remote add origin "https://github.com/RyderFreeman4Logos/cli-sub-agent.git"
  git -C "${repo_dir}" remote add origin-ssh "git@github.com:RyderFreeman4Logos/cli-sub-agent.git"
  make_gh_stub "${stub_dir}" "RyderFreeman4Logos"

  resolved_remote="$(
    cd "${repo_dir}" &&
      PATH="${stub_dir}:${PATH}" "${SCRIPT_PATH}" "fix/2406-local-only"
  )"

  if [ "${resolved_remote}" != "origin" ]; then
    echo "expected local-only branch to resolve valid origin, got ${resolved_remote}" >&2
    exit 1
  fi
}

run_no_suitable_remote_error_test() {
  local case_dir="${TMP_ROOT}/no-suitable-remote"
  local repo_dir="${case_dir}/repo"
  local stderr_file="${case_dir}/stderr.txt"

  make_repo "${repo_dir}"
  git -C "${repo_dir}" checkout -b fix/2406-no-remote >/dev/null 2>&1

  set +e
  (
    cd "${repo_dir}"
    "${SCRIPT_PATH}" "fix/2406-no-remote" >"${case_dir}/stdout.txt" 2>"${stderr_file}"
  )
  rc=$?
  set -e

  if [ "${rc}" -ne 1 ]; then
    echo "expected no suitable remote scenario to exit 1, got ${rc}" >&2
    exit 1
  fi

  grep -q "cannot determine a non-empty pr-bot push remote with a push URL" "${stderr_file}"
  grep -q "branch.fix/2406-no-remote.pushRemote: <unset>" "${stderr_file}"
  grep -q "remote.pushDefault: <unset>" "${stderr_file}"
  grep -q "checkout.defaultRemote: <unset>" "${stderr_file}"
  grep -q "git config --local branch.fix/2406-no-remote.pushRemote <name>" "${stderr_file}"
  grep -q "git config remote.pushDefault <name>" "${stderr_file}"
}

run_ambiguous_origin_fail_closed_test
run_explicit_push_remote_wins_test
run_branch_push_remote_wins_over_invalid_push_default_test
run_invalid_branch_push_remote_fail_closed_test
run_invalid_remote_push_default_fail_closed_test
run_local_only_branch_origin_fallback_test
run_no_suitable_remote_error_test

echo "resolve-push-remote tests: PASS"
