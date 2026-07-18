#!/usr/bin/env bash
set -euo pipefail
shopt -s inherit_errexit
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
export GIT_CONFIG_NOSYSTEM=1

repo_root="$(git rev-parse --show-toplevel)"
source "$repo_root/scripts/tests/quality-gate-test-assertions.sh"
source "$repo_root/scripts/hooks/quality-gates-live.sh"
receipt_contract_install_failure_trap pre-push-quality-gates-tests.sh
scenario="${1:-receipt-reuse-with-hard-gates}"
mkdir -p "$repo_root/drafts"
test_root="$(realpath -e "$(mktemp -d "$repo_root/drafts/pre-push-quality-gates.XXXXXX")")"
trap 'rm -rf -- "$test_root"' EXIT

test_static_nextest_profile_contract() {
  receipt_contract_set_case static-nextest-profile
  local config default_filter selector_pattern test_identifier test_leaf
  local count declarations owners
  config="$(<.config/nextest.toml)"
  assert_contains static-profile-preserves-default-retries \
    $'[profile.default]\nretries = 2' "$config"
  assert_contains static-profile-section '[profile.static]' "$config"
  assert_contains static-profile-retries 'retries = 0' "$config"
  assert_contains static-profile-fail-fast 'fail-fast = false' "$config"
  assert_contains static-profile-slow-timeout \
    'slow-timeout = { period = "60s", terminate-after = 2, grace-period = "10s", on-timeout = "fail" }' \
    "$config"

  count="$(grep -Ec '^default-filter = ' .config/nextest.toml || true)"
  assert_eq static-profile-single-selector 1 "$count"
  default_filter="$(sed -nE 's/^default-filter = "(.*)"$/\1/p' .config/nextest.toml)"
  selector_pattern='^not \(binary_id\(=csa-executor\) & test\(=([[:alnum:]_:]+)\)\)$'
  if [[ "$default_filter" =~ $selector_pattern ]]; then
    test_identifier="${BASH_REMATCH[1]}"
  else
    _receipt_test_fail static-profile-exact-selector \
      'not (binary_id(=csa-executor) & test(=<fully-qualified-test>))' \
      "$default_filter"
  fi
  case "$test_identifier" in
    transport::tests::*) ;;
    *)
      _receipt_test_fail static-profile-transport-test \
        'transport::tests::<test>' "$test_identifier"
      ;;
  esac
  test_leaf="${test_identifier##*::}"
  declarations="$(
    git grep -E \
      "^[[:space:]]*(async[[:space:]]+)?fn[[:space:]]+${test_leaf}[[:space:]]*\\(" \
      -- ':(glob)crates/**/*.rs' || true
  )"
  count="$(grep -c . <<<"$declarations" || true)"
  assert_eq static-profile-target-declaration-count 1 "$count"
  owners="$(
    git grep -l -F "$test_identifier" -- \
      ':(glob)**/*.sh' ':(glob)**/*.py' || true
  )"
  assert_empty static-profile-target-owned-only-by-nextest-config "$owners"
  echo 'PASS static-nextest-profile'
}

test_static_gate_invocation_contract() {
  receipt_contract_set_case static-gate-invocation
  local static_source fixture capture observed count
  fixture="$test_root/static-gate-invocation"
  capture="$fixture/just-capture"
  mkdir -p "$fixture/bin" "$fixture/scripts/hooks" "$fixture/scripts"
  cp scripts/hooks/pre-push-quality-gates.sh "$fixture/scripts/hooks/"
  cat >"$fixture/bin/git" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [ "$*" != 'rev-parse --show-toplevel' ]; then
  printf 'unexpected git invocation: %s\n' "$*" >&2
  exit 2
fi
printf '%s\n' "$STATIC_FIXTURE_ROOT"
EOF
  cat >"$fixture/bin/just" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'argv=%s|profile=%s|user-config=%s|retries=%s\n' \
  "$*" "${NEXTEST_PROFILE-<unset>}" "${NEXTEST_USER_CONFIG_FILE-<unset>}" \
  "${NEXTEST_RETRIES-<unset>}" >>"$STATIC_GATE_CAPTURE"
EOF
  cat >"$fixture/scripts/cargo-env-normalize.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
EOF
  chmod +x "$fixture/bin/git" "$fixture/bin/just" \
    "$fixture/scripts/cargo-env-normalize.sh" \
    "$fixture/scripts/hooks/pre-push-quality-gates.sh"
  (
    cd "$fixture"
    PATH="$fixture/bin:$PATH" \
      STATIC_FIXTURE_ROOT="$fixture" \
      STATIC_GATE_CAPTURE="$capture" \
      NEXTEST_PROFILE=hostile \
      NEXTEST_USER_CONFIG_FILE=hostile \
      NEXTEST_RETRIES=not-a-number \
      scripts/hooks/pre-push-quality-gates.sh
  )
  observed="$(grep -F 'argv=test|' "$capture" || true)"
  count="$(grep -Fc 'argv=test|' "$capture" || true)"
  assert_eq static-gate-single-test-invocation 1 "$count"
  assert_eq static-gate-hostile-env-is-pinned \
    'argv=test|profile=static|user-config=none|retries=0' "$observed"

  static_source="$(<scripts/hooks/pre-push-quality-gates.sh)"
  count="$(
    grep -Ec \
      '^NEXTEST_PROFILE=static NEXTEST_USER_CONFIG_FILE=none NEXTEST_RETRIES=0 just test$' \
      <<<"$static_source" || true
  )"
  assert_eq static-gate-pinned-nextest-invocation 1 "$count"
  count="$(grep -Ec '^just test$' <<<"$static_source" || true)"
  assert_eq static-gate-no-unpinned-nextest-invocation 0 "$count"
  echo 'PASS static-gate-invocation'
}

assert_live_invocation_capture() {
  local case_prefix="$1" capture="$2" expected_jobs="$3" shared invocation index
  local -a invocations
  mapfile -t invocations <"$capture"
  assert_eq "${case_prefix}-invocation-count" 4 "${#invocations[@]}"
  shared='<--profile><static><--user-config-file><none><--ignore-default-filter><-E><not default()>'
  for index in "${!invocations[@]}"; do
    invocation="${invocations[$index]}"
    assert_contains "${case_prefix}-${index}-profile" 'profile=static' "$invocation"
    assert_contains "${case_prefix}-${index}-user-config" 'user-config=none' "$invocation"
    assert_contains "${case_prefix}-${index}-retries" 'retries=0' "$invocation"
    assert_contains "${case_prefix}-${index}-double-spawn" 'double-spawn=0' "$invocation"
    assert_contains "${case_prefix}-${index}-build-jobs" \
      "build-jobs=${expected_jobs}" "$invocation"
    assert_contains "${case_prefix}-${index}-shared-selector" "$shared" "$invocation"
  done
  assert_contains "${case_prefix}-default-list" \
    "<list>${shared}<--workspace><--message-format><json>" "$(<"$capture")"
  assert_contains "${case_prefix}-default-run" \
    "<run>${shared}<--workspace><--no-tests><fail><--test-threads><1>" \
    "$(<"$capture")"
  assert_contains "${case_prefix}-all-features-list" \
    "<list>${shared}<--workspace><--all-features><--message-format><json>" \
    "$(<"$capture")"
  assert_contains "${case_prefix}-all-features-run" \
    "<run>${shared}<--workspace><--all-features><--no-tests><fail><--test-threads><1>" \
    "$(<"$capture")"
}

run_live_nextest_fixture_case() {
  local fixture="$1" capture="$2" retries="$3" build_jobs="$4"
  local -a env_options=() env_vars=(
    "LIVE_NEXTEST_CAPTURE=$capture"
    'NEXTEST_PROFILE=hostile'
    'NEXTEST_USER_CONFIG_FILE=hostile'
    "NEXTEST_RETRIES=$retries"
    'NEXTEST_DOUBLE_SPAWN=hostile'
  )
  if [ "$build_jobs" = auto ]; then
    env_options=(-u CARGO_BUILD_JOBS -u FAIL_DETECT_BUILD_JOBS)
  else
    env_vars+=("CARGO_BUILD_JOBS=$build_jobs" 'FAIL_DETECT_BUILD_JOBS=1')
  fi
  (
    cd "$fixture"
    env "${env_options[@]}" "${env_vars[@]}" bash -c "
set -euo pipefail
source \"\$1\"
require_live_cgroup_host() { :; }
run_live_cgroup_tests >/dev/null
" bash "$repo_root/scripts/hooks/quality-gates-live.sh"
  )
}

test_live_selector_and_leg_contract() {
  receipt_contract_set_case live-selector-and-legs
  local live_source fixture auto_capture override_capture count
  fixture="$test_root/live-selector-and-legs"
  auto_capture="$fixture/auto-capture"
  override_capture="$fixture/override-capture"
  mkdir -p "$fixture/scripts"
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
  printf '%s\n' '{"rust-suites":{"suite":{"testcases":{"target":{"filter-match":{"status":"matches"}}}}}}'
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

  live_source="$(<scripts/hooks/quality-gates-live.sh)"
  assert_contains live-selector-profile '--profile static' "$live_source"
  assert_contains live-selector-user-config '--user-config-file none' "$live_source"
  assert_contains live-selector-ignore-default '--ignore-default-filter' "$live_source"
  assert_contains live-selector-complement "-E 'not default()'" "$live_source"
  assert_contains live-run-no-tests-fail '--no-tests fail' "$live_source"
  assert_contains live-run-single-thread '--test-threads 1' "$live_source"
  assert_contains live-default-leg \
    'run_live_cgroup_leg default --workspace' "$live_source"
  assert_contains live-all-features-leg \
    'run_live_cgroup_leg all-features --workspace --all-features' "$live_source"
  count="$(grep -Ec 'nextest list' <<<"$live_source" || true)"
  assert_eq live-single-inventory-path 1 "$count"
  count="$(grep -Ec 'nextest run' <<<"$live_source" || true)"
  assert_eq live-single-execution-path 1 "$count"
  echo 'PASS live-selector-and-legs'
}

test_live_preflight_and_leg_order_contract() {
  receipt_contract_set_case live-preflight-and-leg-order
  local actual failure_code failure_output
  actual="$(
    require_live_cgroup_host() { printf '%s\n' preflight; }
    run_live_cgroup_leg() { printf 'leg:%s\n' "$*"; }
    run_live_cgroup_tests
  )"
  assert_eq live-preflight-and-leg-order \
    $'preflight\nleg:default --workspace\nleg:all-features --workspace --all-features' \
    "$actual"

  set +e
  failure_output="$(
    set +e
    require_live_cgroup_host() { return 1; }
    run_live_cgroup_leg() { printf 'unexpected-leg:%s\n' "$*"; }
    run_live_cgroup_tests
  )"
  failure_code=$?
  set -e
  assert_ne live-preflight-failure-exit 0 "$failure_code"
  assert_empty live-preflight-failure-runs-no-leg "$failure_output"
  echo 'PASS live-preflight-and-leg-order'
}

test_live_cardinality_contract() {
  local matches="$1" expected_code="$2" json code
  receipt_contract_set_case "live-cardinality-${matches}"
  case "$matches" in
    0)
      json='{"test-count":99,"rust-suites":{"suite":{"testcases":{"other":{"filter-match":{"status":"mismatch"}}}}}}'
      ;;
    1)
      json='{"test-count":99,"rust-suites":{"suite":{"testcases":{"target":{"filter-match":{"status":"matches"}},"other":{"filter-match":{"status":"mismatch"}}}}}}'
      ;;
    2)
      json='{"test-count":99,"rust-suites":{"suite":{"testcases":{"target-a":{"filter-match":{"status":"matches"}},"target-b":{"filter-match":{"status":"matches"}}}}}}'
      ;;
    *) echo "invalid synthetic match count: $matches" >&2; return 2 ;;
  esac
  set +e
  printf '%s\n' "$json" \
    | require_single_nextest_match "synthetic-${matches}" >/dev/null 2>&1
  code=$?
  set -e
  if [ "$expected_code" = 0 ]; then
    assert_eq "live-cardinality-${matches}-exit" 0 "$code"
  else
    assert_ne "live-cardinality-${matches}-exit" 0 "$code"
  fi
  echo "PASS live-cardinality-${matches}"
}

require_source_contract() {
  local summary quality_recipe pre_push_recipe static_source contract_source
  local live_source lefthook_source count suite
  summary="$(just --no-dotenv --summary)"
  quality_recipe="$(just --no-dotenv --show quality-gates)"
  pre_push_recipe="$(just --no-dotenv --show pre-push)"
  static_source="$(<scripts/hooks/pre-push-quality-gates.sh)"
  live_source="$(<scripts/hooks/quality-gates-live.sh)"
  contract_source="$(<scripts/hooks/quality-gate-contract-tests.sh)"
  lefthook_source="$(<lefthook.yml)"
  assert_contains source-contract-quality-recipe-listed quality-gates "$summary"
  assert_contains source-contract-quality-recipe-entrypoint \
    scripts/hooks/quality-gates.sh "$quality_recipe"
  assert_contains source-contract-pre-push-hook-mode \
    'CSA_QUALITY_GATE_HOOK_MODE=1 scripts/hooks/quality-gates.sh' "$pre_push_recipe"
  count="$(grep -c 'scripts/hooks/quality-gate-contract-tests.sh' \
    <<<"$live_source" || true)"
  assert_eq source-contract-authoritative-stage-count 1 "$count"
  assert_contains source-contract-deny-is-live 'just deny' "$live_source"
  assert_contains source-contract-monolith-is-live 'scripts/monolith/check.sh' "$live_source"
  assert_not_matches source-contract-deny-not-static 'just deny' "$static_source"
  for suite in \
    quality-gate-receipt-tests.sh \
    quality-gate-receipt-hostile-tests.sh \
    quality-gate-isolation-tests.sh \
    pre-push-quality-gates-tests.sh \
    dev2merge-quality-gate-receipt-tests.sh; do
    count="$(grep -c "scripts/tests/${suite}" <<<"$contract_source" || true)"
    assert_eq "source-contract-${suite}-owner-count" 1 "$count"
  done
  assert_not_matches source-contract-no-recursive-gate \
    'just (quality-gates|pre-push)|scripts/hooks/(quality-gates|pre-push-quality-gates)\.sh' \
    "$contract_source"
  assert_contains source-contract-hook-pre-push 'run: just pre-push' "$lefthook_source"
  assert_contains source-contract-hook-branch-protection \
    'run: scripts/hooks/branch-protection.sh' "$lefthook_source"
  assert_contains source-contract-hook-version-check \
    'run: scripts/hooks/version-check.sh' "$lefthook_source"
  assert_contains source-contract-hook-review-check \
    'run: scripts/hooks/review-check.sh' "$lefthook_source"
}

new_fixture() {
  local fixture="$test_root/repo"
  mkdir -p "$fixture/scripts/hooks" "$fixture/scripts" "$fixture/.csa/state" \
    "$fixture/target/quality-gate-test-state"
  printf '/.csa/state/\n/target/\n' >"$fixture/.gitignore"
  git -C "$fixture" init -q
  git -C "$fixture" config user.name "Pre-push Tests"
  git -C "$fixture" config user.email "pre-push-tests@example.invalid"
  git -C "$fixture" remote add origin https://example.invalid/pre-push.git
  cp "$repo_root/scripts/hooks/quality-gate-receipt.sh" "$fixture/scripts/hooks/"
  cp "$repo_root/scripts/hooks/quality-gates.sh" "$fixture/scripts/hooks/"
  cp "$repo_root/scripts/cargo-env-normalize.sh" "$fixture/scripts/"
  cp "$repo_root/scripts/quality-gate-state.py" "$fixture/scripts/"
  cp "$repo_root/scripts/quality_gate_secure_state.py" "$fixture/scripts/"
  cp "$repo_root/scripts/quality_gate_provenance.py" "$fixture/scripts/"
  cp "$repo_root/scripts/quality_gate_sandbox.py" "$fixture/scripts/"
  cp "$repo_root/scripts/quality_gate_process.py" "$fixture/scripts/"
  cp "$repo_root/scripts/quality_gate_environment.py" "$fixture/scripts/"
  cp "$repo_root/scripts/quality_gate_toolchain.py" "$fixture/scripts/"
  cp "$repo_root/scripts/rename-no-replace.py" "$fixture/scripts/"
  cp "$repo_root/rust-toolchain.toml" "$fixture/"
  printf '[workspace]\n' >"$fixture/Cargo.toml"
  printf '# lock\n' >"$fixture/Cargo.lock"
  printf '# weave\n' >"$fixture/weave.lock"
  printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>target/quality-gate-test-state/quality-counter\n' \
    >"$fixture/scripts/hooks/pre-push-quality-gates.sh"
  printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>target/quality-gate-test-state/live-counter\n' \
    >"$fixture/scripts/hooks/quality-gates-live.sh"
  local gate
  for gate in branch-protection version-check review-check; do
    printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>target/quality-gate-test-state/%s-counter\nif [ -e target/quality-gate-test-state/fail-%s ]; then printf "fixture %s gate forced failure\\n" >&2; exit 1; fi\n' \
      "$gate" "$gate" "$gate" >"$fixture/scripts/hooks/${gate}.sh"
  done
  chmod +x "$fixture/scripts/hooks/"*.sh
  cat >"$fixture/justfile" <<'EOF'
quality-gates:
    scripts/hooks/quality-gates.sh

pre-push:
    CSA_QUALITY_GATE_HOOK_MODE=1 scripts/hooks/quality-gates.sh
EOF
  cp "$repo_root/lefthook.yml" "$fixture/lefthook.yml"
  git -C "$fixture" add .gitignore Cargo.toml Cargo.lock weave.lock justfile \
    lefthook.yml rust-toolchain.toml scripts
  git -C "$fixture" commit -qm "test: initialize pre-push fixture"
  printf '%s\n' "$fixture"
}

run_contract() {
  require_source_contract
  local fixture quality identity gate before code
  local -a receipts
  fixture="$(new_fixture)"
  (cd "$fixture" && QUALITY_GATE_TEST_TOKEN=alpha lefthook run pre-push >/dev/null)
  git -C "$fixture" update-ref refs/heads/main HEAD
  mkdir -p "$fixture/target/debug" "$fixture/target/cargo-deny-advisory-dbs"
  printf first >"$fixture/target/debug/csa"
  printf advisory >"$fixture/target/cargo-deny-advisory-dbs/revision"
  local second_output
  second_output="$(cd "$fixture" && QUALITY_GATE_TEST_TOKEN=beta \
    lefthook run pre-push 2>&1)"
  assert_not_matches receipt-reuse-secret-values-absent 'alpha|beta' "$second_output"
  quality="$(wc -c <"$fixture/target/quality-gate-test-state/quality-counter")"
  assert_eq receipt-reuse-quality-runs 1 "$quality"
  assert_eq receipt-reuse-live-runs 2 \
    "$(wc -c <"$fixture/target/quality-gate-test-state/live-counter")"
  for gate in branch-protection version-check review-check; do
    assert_eq "receipt-reuse-${gate}-runs" 2 \
      "$(wc -c <"$fixture/target/quality-gate-test-state/${gate}-counter")"
  done
  mapfile -t receipts < <(
    find "$fixture/.csa/state/quality-gate-receipts" -maxdepth 1 -type f -name '*.json'
  )
  assert_eq receipt-reuse-receipt-count 1 "${#receipts[@]}"
  identity="$(basename "${receipts[0]}" .json)"
  assert_nonempty receipt-reuse-identity "$identity"

  for gate in branch-protection version-check review-check; do
    before="$(wc -c <"$fixture/target/quality-gate-test-state/${gate}-counter")"
    touch "$fixture/target/quality-gate-test-state/fail-${gate}"
    set +e
    (cd "$fixture" && lefthook run pre-push >/dev/null 2>&1)
    code=$?
    set -e
    rm "$fixture/target/quality-gate-test-state/fail-${gate}"
    assert_ne "hard-gate-${gate}-failure-exit" 0 "$code"
    assert_eq "hard-gate-${gate}-quality-runs" 1 \
      "$(wc -c <"$fixture/target/quality-gate-test-state/quality-counter")"
    assert_eq "hard-gate-${gate}-live-runs" "$((before + 1))" \
      "$(wc -c <"$fixture/target/quality-gate-test-state/live-counter")"
    assert_eq "hard-gate-${gate}-runs" "$((before + 1))" \
      "$(wc -c <"$fixture/target/quality-gate-test-state/${gate}-counter")"
  done
  echo "PASS receipt-reuse-with-hard-gates identity=${identity}"
}

case "$scenario" in
  hostile-nextest-env)
    test_live_selector_and_leg_contract
    test_static_gate_invocation_contract
    ;;
  receipt-reuse-with-hard-gates)
    test_static_nextest_profile_contract
    test_static_gate_invocation_contract
    test_live_selector_and_leg_contract
    test_live_preflight_and_leg_order_contract
    test_live_cardinality_contract 0 1
    test_live_cardinality_contract 1 0
    test_live_cardinality_contract 2 1
    receipt_contract_set_case receipt-reuse-with-hard-gates
    run_contract
    ;;
  *) echo "unknown scenario: $scenario" >&2; exit 2 ;;
esac
