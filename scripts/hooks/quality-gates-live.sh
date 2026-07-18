#!/usr/bin/env bash
# Mutable/current quality checks that must execute on every direct or hook call.
set -euo pipefail

live_cargo_build_jobs=
live_cargo_build_jobs_resolved=0

require_single_nextest_match() {
  local leg="$1" count
  if ! count="$(
    python3 -c '
import json
import sys

try:
    inventory = json.load(sys.stdin)
    suites = inventory.get("rust-suites")
    if not isinstance(suites, dict):
        raise ValueError("rust-suites must be an object")
    count = 0
    for suite in suites.values():
        if not isinstance(suite, dict):
            raise ValueError("suite must be an object")
        testcases = suite.get("testcases")
        if not isinstance(testcases, dict):
            raise ValueError("testcases must be an object")
        for testcase in testcases.values():
            if not isinstance(testcase, dict):
                raise ValueError("testcase must be an object")
            filter_match = testcase.get("filter-match")
            if (
                isinstance(filter_match, dict)
                and filter_match.get("status") == "matches"
            ):
                count += 1
except (json.JSONDecodeError, ValueError) as error:
    print(f"ERROR: invalid nextest inventory: {error}", file=sys.stderr)
    raise SystemExit(2)

print(count)
'
  )"; then
    printf 'ERROR: unable to inspect live Cgroup inventory for %s.\n' "$leg" >&2
    return 1
  fi
  if [ "$count" -ne 1 ]; then
    printf 'ERROR: live Cgroup inventory for %s matched %s tests; expected exactly 1.\n' \
      "$leg" "$count" >&2
    return 1
  fi
  printf 'Live Cgroup inventory for %s: exactly 1 matching test.\n' "$leg"
}

resolve_live_cargo_build_jobs() {
  if [[ -v CARGO_BUILD_JOBS ]]; then
    live_cargo_build_jobs="$CARGO_BUILD_JOBS"
  elif ! live_cargo_build_jobs="$(scripts/detect-build-jobs.sh)"; then
    echo 'ERROR: unable to detect memory-aware Cargo build parallelism.' >&2
    return 1
  fi
  live_cargo_build_jobs_resolved=1
}

run_live_nextest() {
  local action="$1"
  shift
  if [ "$live_cargo_build_jobs_resolved" != 1 ]; then
    if ! resolve_live_cargo_build_jobs; then
      return 1
    fi
  fi
  CARGO_BUILD_JOBS="$live_cargo_build_jobs" \
    NEXTEST_PROFILE=static \
    NEXTEST_USER_CONFIG_FILE=none \
    NEXTEST_RETRIES=0 \
    NEXTEST_DOUBLE_SPAWN=0 \
    scripts/cargo-env-normalize.sh cargo nextest "$action" \
      --profile static \
      --user-config-file none \
      --ignore-default-filter \
      -E 'not default()' \
      "$@"
}

require_live_cgroup_host() {
  local controllers=/sys/fs/cgroup/cgroup.controllers
  if [ ! -f "$controllers" ]; then
    printf 'ERROR: CgroupV2 controllers are unavailable at %s.\n' \
      "$controllers" >&2
    return 1
  fi
  if ! command -v timeout >/dev/null 2>&1; then
    echo 'ERROR: timeout is required for the live Cgroup host preflight.' >&2
    return 1
  fi
  if ! command -v systemd-run >/dev/null 2>&1; then
    echo 'ERROR: systemd-run is required for the live Cgroup host preflight.' >&2
    return 1
  fi
  if ! timeout --signal=TERM --kill-after=5s 20s \
    systemd-run --user --scope --quiet /bin/true; then
    echo 'ERROR: bounded user systemd scope preflight failed.' >&2
    return 1
  fi
}

run_live_cgroup_leg() {
  local leg="$1"
  shift
  local -a workspace_args=("$@")

  printf 'Inventorying live Cgroup test for %s...\n' "$leg"
  if ! run_live_nextest list \
    "${workspace_args[@]}" --message-format json \
    | require_single_nextest_match "$leg"; then
    return 1
  fi
  printf 'Running live Cgroup test for %s...\n' "$leg"
  run_live_nextest run \
    "${workspace_args[@]}" \
    --no-tests fail --test-threads 1
}

run_live_cgroup_tests() {
  live_cargo_build_jobs_resolved=0
  if ! resolve_live_cargo_build_jobs; then
    return 1
  fi
  if ! require_live_cgroup_host; then
    return 1
  fi
  if ! run_live_cgroup_leg default --workspace; then
    return 1
  fi
  run_live_cgroup_leg all-features --workspace --all-features
}

quality_gates_live_main() {
  local repo_root
  repo_root="$(git rev-parse --show-toplevel)"
  cd "$repo_root"

  scripts/monolith/check.sh --scope all --baseline scripts/monolith/baseline.toml --report-all
  just check-path-includes
  if [ "${CSA_QUALITY_GATE_HOOK_MODE:-0}" != "1" ]; then
    just check-version-bumped
  fi
  ./scripts/hooks/check-env-dependent-tests.sh
  run_live_cgroup_tests
  scripts/hooks/quality-gate-contract-tests.sh
  # Advisory databases and policy/network freshness are deliberately live. They
  # are never authenticated by, or skipped because of, the static-stage receipt.
  just deny
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  quality_gates_live_main "$@"
fi
