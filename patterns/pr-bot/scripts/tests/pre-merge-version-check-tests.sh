#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel)"
SCRIPT_PATH="${ROOT_DIR}/patterns/pr-bot/scripts/pre-merge-version-check.sh"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

write_cargo_version() {
  local repo_dir="$1"
  local version="$2"
  cat >"${repo_dir}/Cargo.toml" <<EOF
[workspace]
members = []

[workspace.package]
version = "${version}"
edition = "2024"
EOF
}

make_fake_just() {
  local stub_dir="$1"
  mkdir -p "${stub_dir}"
  cat >"${stub_dir}/just" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

read_version() {
  sed -n 's/^version = "\(.*\)"/\1/p' "$1" | head -1
}

case "${1:-}" in
  --summary)
    echo "check-version-bumped"
    ;;
  check-version-bumped)
    current="$(read_version Cargo.toml)"
    main_version="$(git show main:Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)"
    if [ "${current}" = "${main_version}" ]; then
      echo "Version (${current}) matches main. Run 'just bump-patch' before pushing." >&2
      exit 1
    fi
    ;;
  *)
    echo "unexpected just invocation: $*" >&2
    exit 2
    ;;
esac
EOF
  chmod +x "${stub_dir}/just"
}

make_fake_just_without_version_target() {
  local stub_dir="$1"
  mkdir -p "${stub_dir}"
  cat >"${stub_dir}/just" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
  --summary)
    echo "fmt test"
    ;;
  *)
    echo "unexpected just invocation: $*" >&2
    exit 2
    ;;
esac
EOF
  chmod +x "${stub_dir}/just"
}

make_fake_just_summary_failure() {
  local stub_dir="$1"
  mkdir -p "${stub_dir}"
  cat >"${stub_dir}/just" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
  --summary)
    echo "error: failed to parse justfile" >&2
    exit 1
    ;;
  *)
    echo "unexpected just invocation: $*" >&2
    exit 2
    ;;
esac
EOF
  chmod +x "${stub_dir}/just"
}

make_repo() {
  local case_dir="$1"
  local repo_dir="${case_dir}/repo"
  local remote_dir="${case_dir}/remote"

  mkdir -p "${remote_dir}"
  git init "${remote_dir}" >/dev/null 2>&1
  git -C "${remote_dir}" config user.name "Remote User"
  git -C "${remote_dir}" config user.email "remote@example.com"
  write_cargo_version "${remote_dir}" "1.0.0"
  git -C "${remote_dir}" add Cargo.toml
  git -C "${remote_dir}" commit -m "init" >/dev/null 2>&1
  git -C "${remote_dir}" branch -M main

  git clone "${remote_dir}" "${repo_dir}" >/dev/null 2>&1
  git -C "${repo_dir}" config user.name "Test User"
  git -C "${repo_dir}" config user.email "test@example.com"
  git -C "${repo_dir}" checkout -b fix/version-gate >/dev/null 2>&1

  printf '%s\n' "${repo_dir}"
}

advance_remote_main() {
  local case_dir="$1"
  local remote_dir="${case_dir}/remote"
  local version="$2"

  write_cargo_version "${remote_dir}" "${version}"
  git -C "${remote_dir}" add Cargo.toml
  git -C "${remote_dir}" commit -m "bump main" >/dev/null 2>&1
}

assert_main_version() {
  local repo_dir="$1"
  local expected="$2"
  local actual
  actual="$(git -C "${repo_dir}" show main:Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)"
  if [ "${actual}" != "${expected}" ]; then
    echo "expected local main version ${expected}, got ${actual}" >&2
    exit 1
  fi
}

run_stale_main_blocks_test() {
  local case_dir="${TMP_ROOT}/stale-main-blocks"
  local stub_dir="${case_dir}/bin"
  local repo_dir
  local stderr_file="${case_dir}/stderr.txt"
  mkdir -p "${case_dir}"
  repo_dir="$(make_repo "${case_dir}")"
  make_fake_just "${stub_dir}"

  write_cargo_version "${repo_dir}" "1.0.1"
  git -C "${repo_dir}" add Cargo.toml
  git -C "${repo_dir}" commit -m "feature bump" >/dev/null 2>&1
  advance_remote_main "${case_dir}" "1.0.1"

  set +e
  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" bash "${SCRIPT_PATH}" origin main
  ) >"${case_dir}/stdout.txt" 2>"${stderr_file}"
  rc=$?
  set -e

  if [ "${rc}" -eq 0 ]; then
    echo "expected stale-main version gate to block" >&2
    exit 1
  fi
  grep -q "BLOCKED: pre-merge version bump gate failed" "${stderr_file}"
  grep -q "Run:  just bump-patch" "${stderr_file}"
  assert_main_version "${repo_dir}" "1.0.1"
}

run_bumped_version_passes_test() {
  local case_dir="${TMP_ROOT}/bumped-version-passes"
  local stub_dir="${case_dir}/bin"
  local repo_dir
  local stderr_file="${case_dir}/stderr.txt"
  mkdir -p "${case_dir}"
  repo_dir="$(make_repo "${case_dir}")"
  make_fake_just "${stub_dir}"

  advance_remote_main "${case_dir}" "1.0.1"
  write_cargo_version "${repo_dir}" "1.0.2"
  git -C "${repo_dir}" add Cargo.toml
  git -C "${repo_dir}" commit -m "feature bump" >/dev/null 2>&1

  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" bash "${SCRIPT_PATH}" origin main
  ) >"${case_dir}/stdout.txt" 2>"${stderr_file}"

  if grep -q "BLOCKED: pre-merge version bump gate failed" "${stderr_file}"; then
    echo "green path unexpectedly emitted block diagnostic" >&2
    exit 1
  fi
  assert_main_version "${repo_dir}" "1.0.1"
}

run_missing_version_target_skips_test() {
  local case_dir="${TMP_ROOT}/missing-version-target-skips"
  local stub_dir="${case_dir}/bin"
  local repo_dir
  local stdout_file="${case_dir}/stdout.txt"
  local stderr_file="${case_dir}/stderr.txt"
  mkdir -p "${case_dir}"
  repo_dir="$(make_repo "${case_dir}")"
  make_fake_just_without_version_target "${stub_dir}"

  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" bash "${SCRIPT_PATH}" origin main
  ) >"${stdout_file}" 2>"${stderr_file}"

  grep -q "pr-bot version gate skipped: just target 'check-version-bumped' is unavailable" "${stdout_file}"
  if [ -s "${stderr_file}" ]; then
    echo "missing version target skip unexpectedly wrote stderr" >&2
    cat "${stderr_file}" >&2
    exit 1
  fi
}

run_missing_just_skips_test() {
  local case_dir="${TMP_ROOT}/missing-just-skips"
  local repo_dir
  local bash_bin
  local stdout_file="${case_dir}/stdout.txt"
  local stderr_file="${case_dir}/stderr.txt"
  mkdir -p "${case_dir}"
  mkdir -p "${case_dir}/no-just-bin"
  repo_dir="$(make_repo "${case_dir}")"
  bash_bin="$(command -v bash)"

  (
    cd "${repo_dir}"
    PATH="${case_dir}/no-just-bin" "${bash_bin}" "${SCRIPT_PATH}" origin main
  ) >"${stdout_file}" 2>"${stderr_file}"

  grep -q "pr-bot version gate skipped: 'just' is unavailable" "${stdout_file}"
  if [ -s "${stderr_file}" ]; then
    echo "missing just skip unexpectedly wrote stderr" >&2
    cat "${stderr_file}" >&2
    exit 1
  fi
}

run_required_missing_just_blocks_test() {
  local case_dir="${TMP_ROOT}/required-missing-just-blocks"
  local repo_dir
  local bash_bin
  local stdout_file="${case_dir}/stdout.txt"
  local stderr_file="${case_dir}/stderr.txt"
  mkdir -p "${case_dir}"
  mkdir -p "${case_dir}/no-just-bin"
  repo_dir="$(make_repo "${case_dir}")"
  bash_bin="$(command -v bash)"

  set +e
  (
    cd "${repo_dir}"
    PATH="${case_dir}/no-just-bin" CSA_REQUIRE_VERSION_CHECK=1 "${bash_bin}" "${SCRIPT_PATH}" origin main
  ) >"${stdout_file}" 2>"${stderr_file}"
  rc=$?
  set -e

  if [ "${rc}" -eq 0 ]; then
    echo "expected required missing just to block" >&2
    exit 1
  fi
  grep -q "CSA_REQUIRE_VERSION_CHECK=1 but 'just' is unavailable" "${stderr_file}"
  grep -q "Install just or unset CSA_REQUIRE_VERSION_CHECK" "${stderr_file}"
}

run_required_missing_version_target_blocks_test() {
  local case_dir="${TMP_ROOT}/required-missing-version-target-blocks"
  local stub_dir="${case_dir}/bin"
  local repo_dir
  local stdout_file="${case_dir}/stdout.txt"
  local stderr_file="${case_dir}/stderr.txt"
  mkdir -p "${case_dir}"
  repo_dir="$(make_repo "${case_dir}")"
  make_fake_just_without_version_target "${stub_dir}"

  set +e
  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" CSA_REQUIRE_VERSION_CHECK=1 bash "${SCRIPT_PATH}" origin main
  ) >"${stdout_file}" 2>"${stderr_file}"
  rc=$?
  set -e

  if [ "${rc}" -eq 0 ]; then
    echo "expected required missing version target to block" >&2
    exit 1
  fi
  grep -q "CSA_REQUIRE_VERSION_CHECK=1 but just target 'check-version-bumped' is unavailable" "${stderr_file}"
  grep -q "Add a local just target named 'check-version-bumped'" "${stderr_file}"
}

run_just_summary_failure_blocks_when_justfile_exists_test() {
  local case_dir="${TMP_ROOT}/summary-failure-blocks"
  local stub_dir="${case_dir}/bin"
  local repo_dir
  local stderr_file="${case_dir}/stderr.txt"
  mkdir -p "${case_dir}"
  repo_dir="$(make_repo "${case_dir}")"
  make_fake_just_summary_failure "${stub_dir}"
  printf 'check-version-bumped:\n' >"${repo_dir}/justfile"

  set +e
  (
    cd "${repo_dir}"
    PATH="${stub_dir}:${PATH}" bash "${SCRIPT_PATH}" origin main
  ) >"${case_dir}/stdout.txt" 2>"${stderr_file}"
  rc=$?
  set -e

  if [ "${rc}" -eq 0 ]; then
    echo "expected just summary failure to block when justfile exists" >&2
    exit 1
  fi
  grep -q "Could not inspect just targets before the pre-merge version gate" "${stderr_file}"
  grep -q "failed to parse justfile" "${stderr_file}"
}

run_stale_main_blocks_test
run_bumped_version_passes_test
run_missing_version_target_skips_test
run_missing_just_skips_test
run_required_missing_just_blocks_test
run_required_missing_version_target_blocks_test
run_just_summary_failure_blocks_when_justfile_exists_test

echo "pre-merge-version-check tests: PASS"
