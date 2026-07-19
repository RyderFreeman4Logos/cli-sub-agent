#!/usr/bin/env bash
# Mutable/current quality checks that must execute on every direct or hook call.
set -euo pipefail

live_cargo_build_jobs=
live_cargo_build_jobs_resolved=0
live_partition_validator=scripts/hooks/quality-gates-live-partition.py

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
  local partition="$2"
  shift 2
  local -a filter_args=()
  case "$partition" in
    all) filter_args=(--ignore-default-filter) ;;
    static) ;;
    live) filter_args=(--ignore-default-filter -E 'not default()') ;;
    *) printf 'ERROR: unknown nextest partition: %s\n' "$partition" >&2; return 2 ;;
  esac
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
      "${filter_args[@]}" \
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

require_live_filesystem_host() {
  if ! command -v timeout >/dev/null 2>&1; then
    echo 'ERROR: timeout is required for the live filesystem host preflight.' >&2
    return 1
  fi
  if ! command -v unshare >/dev/null 2>&1; then
    echo 'ERROR: unshare is required for the live filesystem host preflight.' >&2
    return 1
  fi
  if ! command -v bwrap >/dev/null 2>&1; then
    echo 'ERROR: bwrap is required for the live filesystem host preflight.' >&2
    return 1
  fi
  if ! timeout --signal=TERM --kill-after=5s 20s \
    unshare --user --map-root-user /bin/true; then
    echo 'ERROR: bounded user-namespace preflight failed.' >&2
    return 1
  fi
  if ! timeout --signal=TERM --kill-after=5s 20s \
    bwrap --die-with-parent --unshare-all --ro-bind / / \
      --dev /dev --proc /proc /bin/true; then
    echo 'ERROR: bounded strict Bubblewrap preflight failed.' >&2
    return 1
  fi
}

require_live_host_capabilities() {
  require_live_cgroup_host && require_live_filesystem_host
}

inventory_live_partition_leg() {
  local leg="$1"
  local inventory_root="$2"
  shift 2
  local -a workspace_args=("$@")
  local all_inventory="$inventory_root/$leg-all.json"
  local static_inventory="$inventory_root/$leg-static.json"
  local live_inventory="$inventory_root/$leg-live.json"

  printf 'Inventorying All/Static/Live partitions for %s...\n' "$leg"
  if ! run_live_nextest list all "${workspace_args[@]}" --message-format json >"$all_inventory"; then
    return 1
  fi
  if ! run_live_nextest list static "${workspace_args[@]}" --message-format json >"$static_inventory"; then
    return 1
  fi
  if ! run_live_nextest list live "${workspace_args[@]}" --message-format json >"$live_inventory"; then
    return 1
  fi
  if ! python3 "$live_partition_validator" validate-inventories \
    --config .config/nextest.toml \
    --leg "$leg" \
    --all "$all_inventory" \
    --static "$static_inventory" \
    --live "$live_inventory" \
    --identities-out "$inventory_root/$leg-live-identities"; then
    return 1
  fi
}

run_live_partition_leg() {
  local leg="$1"
  shift
  printf 'Running exact Live partition for %s...\n' "$leg"
  run_live_nextest run live "$@" --no-tests fail --test-threads 1
}

run_live_partition_tests() (
  live_cargo_build_jobs_resolved=0
  if ! require_live_host_capabilities; then
    return 1
  fi
  if ! resolve_live_cargo_build_jobs; then
    return 1
  fi
  local inventory_root
  inventory_root="$(mktemp -d "${TMPDIR:-/tmp}/csa-live-partition.XXXXXX")"
  trap 'rm -rf -- "$inventory_root"' EXIT
  if ! inventory_live_partition_leg default "$inventory_root" --workspace; then
    return 1
  fi
  if ! inventory_live_partition_leg all-features "$inventory_root" --workspace --all-features; then
    return 1
  fi
  if ! cmp -s \
    "$inventory_root/default-live-identities" \
    "$inventory_root/all-features-live-identities"; then
    echo 'ERROR: default/all-features Live identities differ.' >&2
    return 1
  fi
  if ! run_live_partition_leg default --workspace; then
    return 1
  fi
  run_live_partition_leg all-features --workspace --all-features
)

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
  run_live_partition_tests
  scripts/hooks/quality-gate-contract-tests.sh
  # Advisory databases and policy/network freshness are deliberately live. They
  # are never authenticated by, or skipped because of, the static-stage receipt.
  just deny
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  quality_gates_live_main "$@"
fi
