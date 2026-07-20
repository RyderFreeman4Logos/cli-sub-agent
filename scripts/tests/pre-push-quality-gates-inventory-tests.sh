# shellcheck shell=bash
# Live nextest inventory and selector contracts.
# Sourced after the pre-push fixture and assertion helpers are defined.

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  echo 'source-only helper; run: bash scripts/tests/pre-push-quality-gates-tests.sh' >&2
  exit 2
fi

test_live_selector_and_leg_contract() {
  receipt_contract_set_case live-selector-and-legs
  local live_source fixture auto_capture override_capture count code fault
  fixture="$test_root/live-selector-and-legs"
  auto_capture="$fixture/auto-capture"
  override_capture="$fixture/override-capture"
  mkdir -p "$fixture/.config" "$fixture/scripts/hooks"
  cp .config/nextest.toml "$fixture/.config/"
  cp scripts/hooks/quality-gates-live-partition.py "$fixture/scripts/hooks/"
  cat >"$fixture/scripts/detect-build-jobs.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [ "${FAIL_DETECT_BUILD_JOBS:-0}" = 1 ]; then
  echo 'detect-build-jobs must not run when CARGO_BUILD_JOBS is set' >&2
  exit 99
fi
printf '3\n'
EOF
  cat >"$fixture/scripts/cargo-env-normalize.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
{
  printf 'profile=%s|user-config=%s|retries=%s|double-spawn=%s|build-jobs=%s|argv=' \
    "${NEXTEST_PROFILE-<unset>}" "${NEXTEST_USER_CONFIG_FILE-<unset>}" \
    "${NEXTEST_RETRIES-<unset>}" "${NEXTEST_DOUBLE_SPAWN-<unset>}" \
    "${CARGO_BUILD_JOBS-<unset>}"
  printf '<%s>' "$@"
  printf '\n'
} >>"$LIVE_NEXTEST_CAPTURE"
if [ "${3:-}" = list ]; then
  python3 "$LIVE_PARTITION_VALIDATOR" fixture-inventory \
    --fault "$LIVE_PARTITION_FAULT" -- "$@"
fi
EOF
  chmod +x "$fixture/scripts/detect-build-jobs.sh" \
    "$fixture/scripts/cargo-env-normalize.sh"
  run_live_nextest_fixture_case \
    "$fixture" "$auto_capture" not-a-number auto
  assert_live_invocation_capture live-auto-build-jobs "$auto_capture" 3
  run_live_nextest_fixture_case \
    "$fixture" "$override_capture" 999999999999999999999999 7
  assert_live_invocation_capture live-overridden-build-jobs "$override_capture" 7
  run_live_nextest_fixture_case \
    "$fixture" "$fixture/all-features-ignored-capture" 0 2 all-features-ignored
  for fault in live-3 live-5 overlap union-omission default-all-mismatch all-features-all-mismatch identity-drift unknown-status; do
    set +e
    run_live_nextest_fixture_case \
      "$fixture" "$fixture/$fault-capture" 0 2 "$fault" >/dev/null 2>&1
    code=$?
    set -e
    assert_ne "live-partition-rejects-${fault}" 0 "$code"
  done
  live_source="$(<scripts/hooks/quality-gates-live.sh)"
  assert_contains live-selector-ignore-default '--ignore-default-filter' "$live_source"
  assert_contains live-selector-complement "-E 'not default()'" "$live_source"
  assert_contains live-run-no-tests-fail '--no-tests fail' "$live_source"
  assert_contains live-run-single-thread '--test-threads 1' "$live_source"
  assert_contains live-default-leg \
    'run_live_partition_leg default --workspace' "$live_source"
  assert_contains live-all-features-leg \
    'run_live_partition_leg all-features --workspace --all-features' "$live_source"
  count="$(grep -Ec 'run_live_nextest list' <<<"$live_source" || true)"
  assert_eq live-all-static-live-inventory-paths 3 "$count"
  count="$(grep -Ec 'run_live_nextest run' <<<"$live_source" || true)"
  assert_eq live-single-execution-path 1 "$count"
  python3 scripts/hooks/quality-gates-live-partition.py check-function \
    --source crates/csa-executor/src/transport_tests_gemini_fallback_tail.rs \
    --function test_execute_best_effort_sandbox_fallback_preserves_attempt_model_override
  python3 scripts/hooks/quality-gates-live-partition.py check-function \
    --source crates/cli-sub-agent/src/pipeline_sandbox_extra_writable_tests.rs \
    --function test_rust_env_writable_uses_execution_env_instead_of_ambient_cargo_dirs
  python3 scripts/hooks/quality-gates-live-partition.py check-function \
    --source crates/cli-sub-agent/src/pipeline_sandbox_tests_tail.rs \
    --function test_csa_state_paths_in_writable_paths
  python3 scripts/hooks/quality-gates-live-partition.py check-function \
    --source crates/cli-sub-agent/src/pipeline_sandbox_tests_tail.rs \
    --function test_csa_state_paths_survive_replace_semantics
  echo 'PASS live-selector-and-legs'
}
