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

run_stale_main_blocks_test
run_bumped_version_passes_test

echo "pre-merge-version-check tests: PASS"
