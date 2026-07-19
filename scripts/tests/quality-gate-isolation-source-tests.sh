# shellcheck shell=bash
# Ambient-input and host source-exactness contracts for the capability sandbox.
# Sourced after the isolation fixture and assertion helpers are defined.

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  echo 'source-only helper; run: bash scripts/tests/quality-gate-isolation-tests.sh ambient-inputs' >&2
  exit 2
fi

run_ambient_input_isolation() {
  local fixture runner counter first second global_config excludes_file output code
  local just_victim first_identity second_identity external_target
  local masked_native_tool missing_native_tool
  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  mkdir -p "$(dirname "$fixture")/.cargo"
  printf '[build]\nrustflags=["--cfg", "ancestor_injection"]\n' \
    >"$(dirname "$fixture")/.cargo/config.toml"
  cat >"$fixture/scripts/hooks/ambient-probe.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf x >>"$1"
test ! -e ../.cargo/config.toml
test -z "${QUALITY_GATE_TEST_TOKEN:-}"
for name in config config.toml credentials credentials.toml; do
  path="${CARGO_HOME:?}/$name"
  [ ! -s "$path" ]
done
SH
  chmod +x "$fixture/scripts/hooks/ambient-probe.sh"
  git -C "$fixture" add scripts/hooks/ambient-probe.sh
  git -C "$fixture" commit -qm "test: add ambient input probe"
  first="$(cd "$fixture" && QUALITY_GATE_TEST_TOKEN=alpha \
    "$runner" -- scripts/hooks/ambient-probe.sh target/gate-counter)"
  second="$(cd "$fixture" && QUALITY_GATE_TEST_TOKEN=beta \
    "$runner" -- scripts/hooks/ambient-probe.sh target/gate-counter)"
  assert_eq isolation-ambient-first-status executed \
    "$(printf '%s' "$first" | json_field status)"
  assert_eq isolation-ambient-second-status reused \
    "$(printf '%s' "$second" | json_field status)"
  assert_eq isolation-ambient-gate-runs 1 "$(wc -c <"$counter")"
  assert_not_matches isolation-ambient-secret-values 'alpha|beta' "$first$second"

  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  external_target="$test_root/external-symlink-target"
  printf 'external-secret\n' >"$external_target"
  ln -s "$external_target" "$fixture/external-input"
  cat >"$fixture/scripts/hooks/symlink-probe.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf x >>"$1"
test ! -e external-input
if { printf exposed >external-input; } 2>/dev/null; then
  exit 81
fi
SH
  chmod +x "$fixture/scripts/hooks/symlink-probe.sh"
  git -C "$fixture" add external-input scripts/hooks/symlink-probe.sh
  git -C "$fixture" commit -qm "test: add external symlink probe"
  first="$(cd "$fixture" && "$runner" -- scripts/hooks/symlink-probe.sh \
    target/gate-counter)"
  second="$(cd "$fixture" && "$runner" -- scripts/hooks/symlink-probe.sh \
    target/gate-counter)"
  first_identity="$(printf '%s' "$first" | json_field receipt_identity)"
  second_identity="$(printf '%s' "$second" | json_field receipt_identity)"
  if [[ ! "$first_identity" =~ ^[0-9a-f]{64}$ ]]; then
    _receipt_test_fail isolation-external-symlink-identity \
      lowercase-sha256 "$first_identity"
    return 1
  fi
  assert_not_matches isolation-external-symlink-nonzero-identity \
    '^0{64}$' "$first_identity"
  assert_eq isolation-external-symlink-first-status executed \
    "$(printf '%s' "$first" | json_field status)"
  assert_eq isolation-external-symlink-first-reason receipt_missing \
    "$(printf '%s' "$first" | json_field rejection_reason)"
  assert_eq isolation-external-symlink-second-status reused \
    "$(printf '%s' "$second" | json_field status)"
  assert_eq isolation-external-symlink-second-reason None \
    "$(printf '%s' "$second" | json_field rejection_reason)"
  assert_eq isolation-external-symlink-reuse-identity \
    "$first_identity" "$second_identity"
  assert_eq isolation-external-symlink-gate-runs 1 "$(wc -c <"$counter")"
  assert_eq isolation-external-symlink-receipts 1 \
    "$(current_receipt_count "$fixture")"
  assert_path_exists isolation-external-symlink-receipt \
    "$fixture/.csa/state/quality-gate-receipts/${first_identity}.json"
  assert_eq isolation-external-symlink-host external-secret \
    "$(<"$external_target")"

  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  excludes_file="$test_root/global-excludes"
  global_config="$test_root/global-gitconfig"
  printf 'globally-ignored\n' >"$excludes_file"
  git config -f "$global_config" core.excludesFile "$excludes_file"
  cat >"$fixture/scripts/hooks/dirty-gate.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf x >>"$1"
SH
  chmod +x "$fixture/scripts/hooks/dirty-gate.sh"
  git -C "$fixture" add scripts/hooks/dirty-gate.sh
  git -C "$fixture" commit -qm "test: add dirty gate"
  printf 'must-remain-dirty\n' >"$fixture/globally-ignored"
  output="$(cd "$fixture" && GIT_CONFIG_GLOBAL="$global_config" \
    "$runner" -- scripts/hooks/dirty-gate.sh target/gate-counter)"
  assert_eq isolation-global-exclude-status executed \
    "$(printf '%s' "$output" | json_field status)"
  assert_eq isolation-global-exclude-reason dirty_state \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq isolation-global-exclude-receipts 0 "$(current_receipt_count "$fixture")"

  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/nested-just-counter"
  just_victim="$fixture/target/hostile-just-tempdir"
  mkdir -p "$just_victim"
  printf 'checkout-sentinel\n' >"$fixture/checkout-sentinel"
  printf 'victim-sentinel\n' >"$just_victim/sentinel"
  cat >"$fixture/justfile" <<'JUST'
set tempdir := "."

outer:
  #!/usr/bin/env bash
  set -euo pipefail
  test "${JUST_TEMPDIR:?}" = /tmp
  printf outer >"${JUST_TEMPDIR}/outer-policy-probe"
  if { printf unexpected >checkout-sentinel; } 2>/dev/null; then
    exit 71
  fi
  exec just nested

nested:
  #!/usr/bin/env bash
  set -euo pipefail
  test "${JUST_TEMPDIR:?}" = /tmp
  printf nested >"${JUST_TEMPDIR}/nested-policy-probe"
  if { printf unexpected >checkout-sentinel; } 2>/dev/null; then
    exit 72
  fi
  printf x >>target/nested-just-counter
JUST
  git -C "$fixture" add justfile checkout-sentinel
  git -C "$fixture" commit -qm "test: add nested Just static gate"

  first="$(cd "$fixture" && JUST_TEMPDIR="$just_victim" \
    "$runner" -- just outer)"
  second="$(cd "$fixture" && JUST_TEMPDIR="$fixture/target/second-hostile-just-tempdir" \
    "$runner" -- just outer)"
  first_identity="$(printf '%s' "$first" | json_field receipt_identity)"
  second_identity="$(printf '%s' "$second" | json_field receipt_identity)"
  assert_eq isolation-nested-just-first-status executed \
    "$(printf '%s' "$first" | json_field status)"
  assert_eq isolation-nested-just-second-status reused \
    "$(printf '%s' "$second" | json_field status)"
  assert_eq isolation-nested-just-reuse-identity "$first_identity" "$second_identity"
  assert_eq isolation-nested-just-gate-runs 1 "$(wc -c <"$counter")"
  assert_eq isolation-nested-just-checkout-sentinel checkout-sentinel \
    "$(<"$fixture/checkout-sentinel")"
  assert_eq isolation-nested-just-victim-sentinel victim-sentinel \
    "$(<"$just_victim/sentinel")"
  assert_path_absent isolation-nested-just-hostile-outer-probe \
    "$just_victim/outer-policy-probe"
  assert_path_absent isolation-nested-just-hostile-nested-probe \
    "$just_victim/nested-policy-probe"
  assert_eq isolation-nested-just-checkout-no-residue 0 \
    "$(find "$fixture" -maxdepth 1 -name 'just-*' -print | wc -l)"
  assert_eq isolation-nested-just-victim-no-residue 0 \
    "$(find "$just_victim" -name 'just-*' -print | wc -l)"
  assert_no_just_temp_residue isolation-nested-just "$fixture"

  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/native-tool-counter"
  masked_native_tool="$test_root/masked-native-tool"
  missing_native_tool="$test_root/missing-native-tool"
  cat >"$masked_native_tool" <<'SH'
#!/usr/bin/env bash
exit 0
SH
  chmod +x "$masked_native_tool"
  cat >"$fixture/scripts/hooks/native-tool-ambient-probe.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
for variable in CC CXX AR LD CPP; do
  expected="/run/csa-bin/explicit-${variable,,}"
  test "${!variable:?}" = "$expected"
  test -x "$expected"
  "$expected"
done
printf x >>"$1"
SH
  chmod +x "$fixture/scripts/hooks/native-tool-ambient-probe.sh"
  git -C "$fixture" add scripts/hooks/native-tool-ambient-probe.sh
  git -C "$fixture" commit -qm "test: add native tool ambient probe"
  output="$(cd "$fixture" && \
    CC="$masked_native_tool" CXX="$masked_native_tool" \
    AR="$masked_native_tool" LD="$masked_native_tool" \
    CPP="$masked_native_tool" \
    "$runner" -- scripts/hooks/native-tool-ambient-probe.sh \
    target/native-tool-counter)"
  assert_eq isolation-native-tool-masked-status executed \
    "$(printf '%s' "$output" | json_field status)"
  assert_eq isolation-native-tool-masked-runs 1 "$(wc -c <"$counter")"
  assert_eq isolation-native-tool-masked-receipts 1 \
    "$(current_receipt_count "$fixture")"

  set +e
  output="$(cd "$fixture" && CC="$missing_native_tool" \
    "$runner" -- scripts/hooks/native-tool-ambient-probe.sh \
    target/native-tool-counter)"
  code=$?
  set -e
  assert_eq isolation-native-tool-missing-exit 125 "$code"
  assert_eq isolation-native-tool-missing-status gate_failed \
    "$(printf '%s' "$output" | json_field status)"
  assert_eq isolation-native-tool-missing-reason isolation_unavailable \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq isolation-native-tool-missing-no-run 1 "$(wc -c <"$counter")"
  assert_eq isolation-native-tool-missing-no-receipt 1 \
    "$(current_receipt_count "$fixture")"
  echo "PASS isolation-ambient-inputs"
}

assert_exact_source_dirty_pair() {
  local label="$1" fixture="$2" counter="$3" runner first second identity
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  first="$(cd "$fixture" && "$runner" -- scripts/hooks/source-exactness-gate.sh \
    "$counter")"
  second="$(cd "$fixture" && "$runner" -- scripts/hooks/source-exactness-gate.sh \
    "$counter")"
  identity="$(printf '%s' "$first" | json_field receipt_identity)"
  assert_eq "${label}-first-reason" dirty_state \
    "$(printf '%s' "$first" | json_field rejection_reason)"
  assert_eq "${label}-second-reason" dirty_state \
    "$(printf '%s' "$second" | json_field rejection_reason)"
  assert_eq "${label}-gate-runs" 2 "$(wc -c <"$counter")"
  assert_path_absent "${label}-no-receipt" \
    "$fixture/.csa/state/quality-gate-receipts/${identity}.json"
}

prepare_source_exactness_fixture() {
  local fixture="$1" external_target="$2"
  mkdir -p "$fixture/.csa"
  printf 'config-v1\n' >"$fixture/.csa/config.toml"
  printf 'checklist-v1\n' >"$fixture/.csa/review-checklist.md"
  printf 'external-v1\n' >"$external_target"
  ln -s "$external_target" "$fixture/external-input"
  cat >"$fixture/scripts/hooks/source-exactness-gate.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf x >>"$1"
test ! -e .csa/config.toml
test ! -e .csa/review-checklist.md
test ! -e external-input
if { printf exposed >external-input; } 2>/dev/null; then
  exit 81
fi
SH
  chmod +x "$fixture/scripts/hooks/source-exactness-gate.sh"
  git -C "$fixture" add .csa/config.toml .csa/review-checklist.md \
    external-input scripts/hooks/source-exactness-gate.sh
  git -C "$fixture" commit -qm "test: add sanitized source topology"
}


assert_exact_source_dirty_deletion_and_type_change() {
  local fixture runner counter output code
  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  # unstaged deletion of tracked regular file
  rm -f "$fixture/Cargo.toml"
  set +e
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/true-gate.sh target/gate-counter)"
  code=$?
  set -e
  assert_eq source-exactness-deletion-exit 0 "$code"
  assert_eq source-exactness-deletion-status executed \
    "$(printf '%s' "$output" | json_field status)"
  assert_eq source-exactness-deletion-reason dirty_state \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq source-exactness-deletion-gate-count 1 "$(wc -c <"$counter")"
  assert_eq source-exactness-deletion-receipt-count 0 \
    "$(current_receipt_count "$fixture")"

  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  rm -f "$fixture/Cargo.toml"
  ln -s /tmp/quality-gate-type-swap "$fixture/Cargo.toml"
  set +e
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/true-gate.sh target/gate-counter)"
  code=$?
  set -e
  assert_eq source-exactness-type-swap-exit 0 "$code"
  assert_eq source-exactness-type-swap-status executed \
    "$(printf '%s' "$output" | json_field status)"
  assert_eq source-exactness-type-swap-reason dirty_state \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq source-exactness-type-swap-gate-count 1 "$(wc -c <"$counter")"
  assert_eq source-exactness-type-swap-receipt-count 0 \
    "$(current_receipt_count "$fixture")"
  echo "PASS isolation-source-dirty-deletion-type-change"
}


run_source_exactness_contracts() {
  local fixture runner counter external_target first second third identity index_before
  local flags_before index_after flags_after other_target output drift_writer

  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  external_target="$test_root/external-source-exactness"
  prepare_source_exactness_fixture "$fixture" "$external_target"
  index_before="$(sha256sum "$fixture/.git/index")"
  flags_before="$(git -C "$fixture" ls-files -v)"
  first="$(cd "$fixture" && "$runner" -- scripts/hooks/source-exactness-gate.sh \
    "$counter")"
  second="$(cd "$fixture" && "$runner" -- scripts/hooks/source-exactness-gate.sh \
    "$counter")"
  identity="$(printf '%s' "$first" | json_field receipt_identity)"
  assert_not_matches source-exactness-nonzero-identity '^0{64}$' "$identity"
  assert_eq source-exactness-first-status executed \
    "$(printf '%s' "$first" | json_field status)"
  assert_eq source-exactness-first-reason receipt_missing \
    "$(printf '%s' "$first" | json_field rejection_reason)"
  assert_eq source-exactness-second-status reused \
    "$(printf '%s' "$second" | json_field status)"
  assert_eq source-exactness-second-reason None \
    "$(printf '%s' "$second" | json_field rejection_reason)"
  assert_eq source-exactness-reuse-identity "$identity" \
    "$(printf '%s' "$second" | json_field receipt_identity)"
  assert_eq source-exactness-gate-runs 1 "$(wc -c <"$counter")"
  assert_eq source-exactness-receipts 1 "$(current_receipt_count "$fixture")"
  assert_eq source-exactness-external-unchanged external-v1 "$(<"$external_target")"
  index_after="$(sha256sum "$fixture/.git/index")"
  flags_after="$(git -C "$fixture" ls-files -v)"
  assert_eq source-exactness-index-bytes "$index_before" "$index_after"
  assert_eq source-exactness-index-flags "$flags_before" "$flags_after"

  printf 'external-v2\n' >"$external_target"
  third="$(cd "$fixture" && "$runner" -- scripts/hooks/source-exactness-gate.sh \
    "$counter")"
  assert_eq source-exactness-referent-status reused \
    "$(printf '%s' "$third" | json_field status)"
  assert_eq source-exactness-referent-identity "$identity" \
    "$(printf '%s' "$third" | json_field receipt_identity)"
  assert_eq source-exactness-referent-gate-runs 1 "$(wc -c <"$counter")"

  other_target="$test_root/external-source-exactness-other"
  printf 'other\n' >"$other_target"
  rm "$fixture/external-input"
  ln -s "$other_target" "$fixture/external-input"
  rm "$counter"
  assert_exact_source_dirty_pair source-exactness-link-dirty "$fixture" "$counter"
  assert_eq source-exactness-link-dirty-receipt-count 1 \
    "$(current_receipt_count "$fixture")"

  fixture="$(new_isolation_fixture)"
  counter="$fixture/target/gate-counter"
  prepare_source_exactness_fixture "$fixture" "$test_root/assume-external"
  git -C "$fixture" update-index --assume-unchanged Cargo.toml
  printf '[workspacf]\n' >"$fixture/Cargo.toml"
  assert_empty source-exactness-assume-porcelain \
    "$(git -C "$fixture" status --short)"
  if ! git -C "$fixture" diff --quiet -- Cargo.toml; then
    _receipt_test_fail source-exactness-assume-diff hidden visible
    return 1
  fi
  index_before="$(sha256sum "$fixture/.git/index")"
  flags_before="$(git -C "$fixture" ls-files -v Cargo.toml)"
  assert_exact_source_dirty_pair source-exactness-assume "$fixture" "$counter"
  assert_eq source-exactness-assume-index-bytes "$index_before" \
    "$(sha256sum "$fixture/.git/index")"
  assert_eq source-exactness-assume-index-flags "$flags_before" \
    "$(git -C "$fixture" ls-files -v Cargo.toml)"

  fixture="$(new_isolation_fixture)"
  counter="$fixture/target/gate-counter"
  prepare_source_exactness_fixture "$fixture" "$test_root/skip-external"
  git -C "$fixture" update-index --skip-worktree .csa/config.toml
  printf 'config-v2\n' >"$fixture/.csa/config.toml"
  assert_empty source-exactness-skip-porcelain \
    "$(git -C "$fixture" status --short)"
  if ! git -C "$fixture" diff --quiet -- .csa/config.toml; then
    _receipt_test_fail source-exactness-skip-diff hidden visible
    return 1
  fi
  index_before="$(sha256sum "$fixture/.git/index")"
  flags_before="$(git -C "$fixture" ls-files -v .csa/config.toml)"
  assert_exact_source_dirty_pair source-exactness-skip "$fixture" "$counter"
  assert_eq source-exactness-skip-index-bytes "$index_before" \
    "$(sha256sum "$fixture/.git/index")"
  assert_eq source-exactness-skip-index-flags "$flags_before" \
    "$(git -C "$fixture" ls-files -v .csa/config.toml)"

  for output in masked executable staged staged-mode untracked; do
    fixture="$(new_isolation_fixture)"
    counter="$fixture/target/gate-counter"
    prepare_source_exactness_fixture "$fixture" "$test_root/${output}-external"
    case "$output" in
      masked) printf 'config-v2\n' >"$fixture/.csa/config.toml" ;;
      executable) chmod +x "$fixture/Cargo.toml" ;;
      staged) printf 'staged\n' >>"$fixture/Cargo.toml"; git -C "$fixture" add Cargo.toml ;;
      staged-mode) git -C "$fixture" update-index --chmod=+x Cargo.toml ;;
      untracked) printf 'untracked\n' >"$fixture/untracked-source" ;;
    esac
    assert_exact_source_dirty_pair "source-exactness-${output}" "$fixture" "$counter"
  done

  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  prepare_source_exactness_fixture "$fixture" "$test_root/intent-external"
  printf 'intent\n' >"$fixture/intent-to-add"
  git -C "$fixture" add --intent-to-add intent-to-add
  first="$(cd "$fixture" && "$runner" -- scripts/hooks/source-exactness-gate.sh \
    "$counter")"
  second="$(cd "$fixture" && "$runner" -- scripts/hooks/source-exactness-gate.sh \
    "$counter")"
  assert_eq source-exactness-intent-first-reason provenance_invalid \
    "$(printf '%s' "$first" | json_field rejection_reason)"
  assert_eq source-exactness-intent-second-reason provenance_invalid \
    "$(printf '%s' "$second" | json_field rejection_reason)"
  assert_eq source-exactness-intent-gate-runs 2 "$(wc -c <"$counter")"
  assert_eq source-exactness-intent-receipts 0 "$(current_receipt_count "$fixture")"

  for output in masked link; do
    fixture="$(new_isolation_fixture)"
    runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
    counter="$fixture/target/gate-counter"
    prepare_source_exactness_fixture "$fixture" "$test_root/drift-${output}-external"
    cat >"$fixture/scripts/hooks/source-exactness-drift-gate.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf x >>"$1"
printf ready >"$2"
while [ ! -e "$3" ]; do sleep 0.02; done
SH
    chmod +x "$fixture/scripts/hooks/source-exactness-drift-gate.sh"
    git -C "$fixture" add scripts/hooks/source-exactness-drift-gate.sh
    git -C "$fixture" commit -qm "test: add source drift gate"
    (
      cd "$fixture"
      exec "$runner" -- scripts/hooks/source-exactness-drift-gate.sh "$counter" \
        target/drift-ready target/drift-release >target/drift-output.json
    ) &
    drift_writer=$!
    owned_pids+=("$drift_writer")
    wait_for_file "$fixture/target/drift-ready"
    if [ "$output" = masked ]; then
      printf 'config-v2\n' >"$fixture/.csa/config.toml"
    else
      rm "$fixture/external-input"
      ln -s "$test_root/drift-link-other" "$fixture/external-input"
    fi
    touch "$fixture/target/drift-release"
    wait "$drift_writer"
    owned_pids=()
    first="$(<"$fixture/target/drift-output.json")"
    identity="$(printf '%s' "$first" | json_field receipt_identity)"
    assert_eq "source-exactness-${output}-drift-reason" input_drift \
      "$(printf '%s' "$first" | json_field rejection_reason)"
    assert_path_absent "source-exactness-${output}-drift-no-receipt" \
      "$fixture/.csa/state/quality-gate-receipts/${identity}.json"
  done
  assert_exact_source_dirty_deletion_and_type_change

}
