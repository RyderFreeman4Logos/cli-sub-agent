#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
source "$repo_root/scripts/tests/quality-gate-test-assertions.sh"
source_runner="${repo_root}/scripts/hooks/quality-gate-receipt.sh"
scenario="${1:-all}"
test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT
python_executable="$(python3 -c 'import os,sys; print(os.path.realpath(sys.executable))')"
assert_executable fixture-python-launcher "$python_executable"
fixture_launcher_dir="$test_root/fixture-launcher"
mkdir -p "$fixture_launcher_dir"
ln -s "$python_executable" "$fixture_launcher_dir/python3"

new_fixture() {
  local fixture
  fixture="$(mktemp -d "${test_root}/fixture.XXXXXX")"
  git -C "$fixture" init -q
  git -C "$fixture" config user.name "Quality Gate Tests"
  git -C "$fixture" config user.email "quality-gate-tests@example.invalid"
  git -C "$fixture" remote add origin "https://example.invalid/quality-gate.git"
  mkdir -p "$fixture/scripts/hooks" "$fixture/.csa/state"
  cp "${repo_root}/scripts/rename-no-replace.py" "$fixture/scripts/rename-no-replace.py"
  cp "${repo_root}/scripts/cargo-env-normalize.sh" "$fixture/scripts/cargo-env-normalize.sh"
  cp "${repo_root}/scripts/quality-gate-state.py" "$fixture/scripts/quality-gate-state.py"
  cp "${repo_root}/scripts/quality_gate_secure_state.py" \
    "$fixture/scripts/quality_gate_secure_state.py"
  cp "${repo_root}/scripts/quality_gate_provenance.py" \
    "$fixture/scripts/quality_gate_provenance.py"
  cp "$source_runner" "$fixture/scripts/hooks/quality-gate-receipt.sh"
  cp "${repo_root}/rust-toolchain.toml" "$fixture/rust-toolchain.toml"
  printf '[workspace]\n' >"$fixture/Cargo.toml"
  printf '# lock\n' >"$fixture/Cargo.lock"
  printf '# weave\n' >"$fixture/weave.lock"
  printf 'quality-gates:\n    true\n' >"$fixture/justfile"
  printf 'pre-push: {}\n' >"$fixture/lefthook.yml"
  printf '#!/usr/bin/env bash\nset -euo pipefail\ncounter="$1"\nprintf "x" >>"$counter"\n' \
    >"$fixture/scripts/hooks/fake-quality-gate.sh"
  chmod +x "$fixture/scripts/hooks/fake-quality-gate.sh"
  git -C "$fixture" add Cargo.toml Cargo.lock justfile lefthook.yml rust-toolchain.toml scripts
  git -C "$fixture" commit -qm "test: initialize fixture"
  printf '%s\n' "$fixture"
}

json_field() {
  python3 -c '
import json, sys
field = sys.argv[1]
try:
    value = json.load(sys.stdin)[field]
except (json.JSONDecodeError, KeyError, TypeError) as error:
    print(
        f"FAIL json-field-{field} expected=valid-json-field actual={type(error).__name__}",
        file=sys.stderr,
    )
    raise SystemExit(1)
print(value)
' "$1"
}

run_exact_reuse() {
  local fixture counter first second first_status second_status first_identity second_identity
  fixture="$(new_fixture)"
  local runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  counter="${fixture}/.csa/state/gate-counter"

  first="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  second="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  first_status="$(printf '%s' "$first" | json_field status)"
  second_status="$(printf '%s' "$second" | json_field status)"
  first_identity="$(printf '%s' "$first" | json_field receipt_identity)"
  second_identity="$(printf '%s' "$second" | json_field receipt_identity)"

  assert_eq exact-reuse-first-status executed "$first_status"
  assert_eq exact-reuse-second-status reused "$second_status"
  assert_eq exact-reuse-identity "$first_identity" "$second_identity"
  assert_eq exact-reuse-gate-runs 1 "$(wc -c <"$counter")"
  assert_eq exact-reuse-receipt-count 1 \
    "$(find "$fixture/.csa/state/quality-gate-receipts" -type f -name '*.json' | wc -l)"
  assert_empty exact-reuse-fixture-status "$(git -C "$fixture" status --short)"
  echo "PASS exact-reuse"
}

receipt_manifest() {
  local fixture="$1" receipt
  receipt="$(find "$fixture/.csa/state/quality-gate-receipts" -type f -name '*.json' | head -1)"
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["manifest"], end="")' "$receipt"
}

assert_manifest_contract() {
  local fixture counter runner manifest key
  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  (cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter") >/dev/null
  manifest="$(receipt_manifest "$fixture")"
  for key in \
    repository_identity checkout_identity head_oid tree_oid index_oid \
    index_clean tracked_worktree_clean untracked_worktree_digest \
    cargo_lock_sha256 weave_lock_sha256 rust_toolchain_sha256 rust_toolchain_file_sha256 \
    target_provenance_sha256 feature_matrix_sha256 environment_sha256 \
    cargo_config_sha256 dotenv_sha256 normalizer_sha256 tool_provenance_sha256 \
    justfile_sha256 lefthook_sha256 gate_script_sha256 recipe_sha256 \
    implementation_sha256 quality_gate_state_helper_sha256 \
    quality_gate_secure_state_sha256 \
    quality_gate_provenance_sha256 \
    schema_version implementation_version; do
    if ! grep -q "^${key}=" <<<"$manifest"; then
      _receipt_test_fail "manifest-contract-${key}" field-present field-missing
      return 1
    fi
  done
}

invoke_identity() {
  local fixture="$1" counter="$2" runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  shift 2
  (cd "$fixture" && env "MISE_DATA_DIR=${test_root}/mise-default" \
    "PATH=${fixture_launcher_dir}:${PATH}" "$@" \
    "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter") |
    json_field receipt_identity
}

assert_invalidation() {
  local name="$1" mutation="$2" first_env="${3:-}" second_env="${4:-}"
  local fixture counter first second
  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  if [ -n "$first_env" ]; then
    first="$(invoke_identity "$fixture" "$counter" "$first_env")"
  else
    first="$(invoke_identity "$fixture" "$counter")"
  fi
  eval "$mutation"
  if [ -n "$second_env" ]; then
    second="$(invoke_identity "$fixture" "$counter" "$second_env")"
  else
    second="$(invoke_identity "$fixture" "$counter")"
  fi
  assert_ne "invalidation-${name}-identity" "$first" "$second"
  assert_eq "invalidation-${name}-gate-runs" 2 "$(wc -c <"$counter")"
  echo "PASS invalidation-$name"
}

run_path_toolchain_invalidation() {
  local fixture counter first second runs toolchain toolchain_root
  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  for toolchain in toolchain-a toolchain-b; do
    toolchain_root="$test_root/$toolchain"
    mkdir -p "$toolchain_root/bin"
    cat >"$toolchain_root/bin/rustc" <<EOF
#!/usr/bin/env bash
case "\${1:-}" in
  -vV)
    printf 'rustc 1.99.0\\nbinary: $toolchain\\nhost: x86_64-unknown-linux-gnu\\n'
    ;;
  --print)
    if [ "\${2:-}" != sysroot ]; then
      printf 'fixture rustc expected --print sysroot, got: %s\n' "\${2:-unset}" >&2
      exit 64
    fi
    printf '%s\\n' "$toolchain_root"
    ;;
  *) exit 64 ;;
esac
EOF
    chmod +x "$toolchain_root/bin/rustc"
    # Keep the public shell entrypoint from resolving Python through a mise shim
    # that can reorder PATH before the fixture compiler is observed.
    ln -s "$python_executable" "$toolchain_root/bin/python3"
  done
  first="$(invoke_identity "$fixture" "$counter" "PATH=${test_root}/toolchain-a/bin:${PATH}")"
  second="$(invoke_identity "$fixture" "$counter" "PATH=${test_root}/toolchain-b/bin:${PATH}")"
  assert_ne invalidation-toolchain-identity "$first" "$second"
  runs="$(wc -c <"$counter")"
  assert_eq invalidation-toolchain-gate-runs 2 "$runs"
  echo "PASS invalidation-toolchain"
}

run_mise_data_dir_invalidation() {
  assert_invalidation mise-data-dir ':' \
    "MISE_DATA_DIR=${test_root}/mise-a" "MISE_DATA_DIR=${test_root}/mise-b"
}

run_invalidation_matrix() {
  assert_manifest_contract
  assert_invalidation head 'printf "head\n" >"$fixture/head"; git -C "$fixture" add head; git -C "$fixture" commit -qm "test: change head"'
  assert_invalidation index 'printf "index\n" >"$fixture/index"; git -C "$fixture" add index'
  assert_invalidation tracked-worktree 'printf "dirty\n" >>"$fixture/Cargo.toml"'
  assert_invalidation untracked-worktree 'printf "untracked\n" >"$fixture/untracked"'
  assert_invalidation repository 'mv "$fixture/.git" "$fixture/.git-store"; printf "gitdir: .git-store\\n" >"$fixture/.git"'
  assert_invalidation checkout 'moved="${fixture}.moved"; mv "$fixture" "$moved"; fixture="$moved"; counter="${fixture}/.csa/state/gate-counter"'
  assert_invalidation cargo-lock 'printf "changed\n" >>"$fixture/Cargo.lock"'
  assert_invalidation weave-lock 'printf "changed\n" >>"$fixture/weave.lock"'
  assert_invalidation rust-toolchain-file \
    'printf "# changed toolchain contract\n" >>"$fixture/rust-toolchain.toml"'
  run_path_toolchain_invalidation
  local fixture counter first second target_spec
  run_mise_data_dir_invalidation
  assert_invalidation rustc ':' \
    "RUSTC=${test_root}/toolchain-a/bin/rustc" "RUSTC=${test_root}/toolchain-b/bin/rustc"
  printf '#!/usr/bin/env bash\nexec "$@"\n' >"$test_root/wrapper-a"
  printf '#!/usr/bin/env bash\n# changed wrapper\nexec "$@"\n' >"$test_root/wrapper-b"
  chmod +x "$test_root/wrapper-a" "$test_root/wrapper-b"
  assert_invalidation rustc-wrapper ':' \
    "RUSTC_WRAPPER=${test_root}/wrapper-a" "RUSTC_WRAPPER=${test_root}/wrapper-b"
  assert_invalidation rustc-workspace-wrapper ':' \
    "RUSTC_WORKSPACE_WRAPPER=${test_root}/wrapper-a" \
    "RUSTC_WORKSPACE_WRAPPER=${test_root}/wrapper-b"
  assert_invalidation cargo-encoded-rustflags ':' \
    $'CARGO_ENCODED_RUSTFLAGS=-Copt-level=1\x1f-Cdebuginfo=0' \
    $'CARGO_ENCODED_RUSTFLAGS=-Copt-level=2\x1f-Cdebuginfo=0'
  assert_invalidation cargo-deny-disable-fetch ':' \
    'CARGO_DENY_DISABLE_FETCH=0' 'CARGO_DENY_DISABLE_FETCH=1'
  assert_invalidation cargo-deny-offline ':' \
    'CARGO_DENY_OFFLINE=0' 'CARGO_DENY_OFFLINE=1'
  assert_invalidation cargo-net-offline ':' \
    'CARGO_NET_OFFLINE=false' 'CARGO_NET_OFFLINE=true'
  assert_invalidation nextest-profile ':' \
    'NEXTEST_PROFILE=default' 'NEXTEST_PROFILE=ci'
  assert_invalidation nextest-test-threads ':' \
    'NEXTEST_TEST_THREADS=1' 'NEXTEST_TEST_THREADS=2'
  assert_invalidation rustc-bootstrap ':' \
    'RUSTC_BOOTSTRAP=0' 'RUSTC_BOOTSTRAP=1'
  assert_invalidation cargo-home ':' \
    "CARGO_HOME=${test_root}/cargo-home-a" "CARGO_HOME=${test_root}/cargo-home-b"
  assert_invalidation target ':' '' 'CARGO_BUILD_TARGET=other-linux-target'
  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  target_spec="$test_root/custom-target.json"
  printf '{"arch":"x86_64"}\n' >"$target_spec"
  first="$(invoke_identity "$fixture" "$counter" "CARGO_BUILD_TARGET=${target_spec}")"
  printf '{"arch":"aarch64"}\n' >"$target_spec"
  second="$(invoke_identity "$fixture" "$counter" "CARGO_BUILD_TARGET=${target_spec}")"
  assert_ne invalidation-target-spec-bytes-identity "$first" "$second"
  assert_eq invalidation-target-spec-bytes-gate-runs 2 "$(wc -c <"$counter")"
  echo "PASS invalidation-target-spec-bytes"
  assert_invalidation feature-matrix ':' 'CSA_QUALITY_GATE_FEATURE_MATRIX=default' 'CSA_QUALITY_GATE_FEATURE_MATRIX=all-features'
  assert_invalidation environment ':' 'RUSTFLAGS=-Copt-level=1' 'RUSTFLAGS=-Copt-level=2'
  assert_invalidation dotenv 'printf "CARGO_DENY_OFFLINE=true\\n" >"$fixture/.env"'
  assert_invalidation recipe 'printf "changed\n" >>"$fixture/justfile"'
  assert_invalidation implementation 'printf "# changed\n" >>"$fixture/scripts/hooks/quality-gate-receipt.sh"'

  local runner before after
  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  before="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter" | json_field receipt_identity)"
  printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf "x" >>"$1"\nprintf "drift\\n" >>Cargo.toml\n' >"$fixture/scripts/hooks/drift-gate.sh"
  chmod +x "$fixture/scripts/hooks/drift-gate.sh"
  git -C "$fixture" add scripts/hooks/drift-gate.sh
  git -C "$fixture" commit -qm "test: add drift gate"
  after="$(cd "$fixture" && "$runner" -- scripts/hooks/drift-gate.sh "$counter")"
  local after_identity
  after_identity="$(printf '%s' "$after" | json_field receipt_identity)"
  assert_eq invalidation-input-drift-reason input_drift \
    "$(printf '%s' "$after" | json_field rejection_reason)"
  assert_path_absent invalidation-input-drift-no-receipt \
    "$fixture/.csa/state/quality-gate-receipts/${after_identity}.json"
  assert_ne invalidation-input-drift-identity "$before" "$after_identity"
  echo "PASS invalidation-input-drift"
}

current_receipt() {
  find "$1/.csa/state/quality-gate-receipts" -maxdepth 1 -type f -name '*.json' | head -1
}

assert_corruption_reexecutes() {
  local name="$1" mutation="$2" fixture counter runner receipt output
  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  (cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter") >/dev/null
  receipt="$(current_receipt "$fixture")"
  eval "$mutation"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq "integrity-${name}-status" executed \
    "$(printf '%s' "$output" | json_field status)"
  assert_nonempty "integrity-${name}-reason" \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq "integrity-${name}-gate-runs" 2 "$(wc -c <"$counter")"
  echo "PASS integrity-$name"
}

assert_single_json() {
  local record="$1" status
  assert_eq structured-result-line-count 1 "$(printf '%s\n' "$record" | wc -l)"
  status="$(printf '%s' "$record" | json_field status)"
  case "$status" in
    executed | reused | gate_failed) ;;
    *) _receipt_test_fail structured-result-status allowed-status "$status" ;;
  esac
  assert_not_matches structured-result-redaction \
    '/tmp/|example\.invalid|credential|secret-token' "$record"
}

wait_for_pid_bounded() {
  local pid="$1"
  if ! timeout 10 tail --pid="$pid" -f /dev/null; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    _receipt_test_fail fixture-process-timeout completed-within-10s timed-out
    return 1
  fi
  local code
  if wait "$pid"; then
    return 0
  else
    code=$?
  fi
  _receipt_test_fail fixture-process-exit 0 "$code"
}

run_integrity_concurrency() {
  assert_corruption_reexecutes malformed 'printf "{truncated" >"$receipt"'
  assert_corruption_reexecutes unknown-schema 'python3 - "$receipt" <<'"'"'PY'"'"'
import json,sys
p=sys.argv[1]; value=json.load(open(p)); value["schema_version"]=999; open(p,"w").write(json.dumps(value))
PY'
  assert_corruption_reexecutes missing-field 'python3 - "$receipt" <<'"'"'PY'"'"'
import json,sys
p=sys.argv[1]; value=json.load(open(p)); del value["status"]; open(p,"w").write(json.dumps(value))
PY'
  assert_corruption_reexecutes non-pass 'python3 - "$receipt" <<'"'"'PY'"'"'
import json,sys
p=sys.argv[1]; value=json.load(open(p)); value["status"]="FAIL"; open(p,"w").write(json.dumps(value))
PY'
  assert_corruption_reexecutes content-digest 'python3 - "$receipt" <<'"'"'PY'"'"'
import json,sys
p=sys.argv[1]; value=json.load(open(p)); value["receipt_digest"]="0"*64; open(p,"w").write(json.dumps(value))
PY'
  assert_corruption_reexecutes filename-digest 'python3 - "$receipt" <<'"'"'PY'"'"'
import json,sys
p=sys.argv[1]; value=json.load(open(p)); value["identity"]="f"*64; open(p,"w").write(json.dumps(value))
PY'
  assert_corruption_reexecutes symlink 'target="${receipt}.target"; mv "$receipt" "$target"; ln -s "$target" "$receipt"'
  assert_corruption_reexecutes non-file 'rm -f "$receipt"; mkdir "$receipt"'

  local fixture counter runner output code receipt_dir
  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  printf '#!/usr/bin/env bash\nexit 7\n' >"$fixture/scripts/hooks/failing-gate.sh"
  chmod +x "$fixture/scripts/hooks/failing-gate.sh"
  git -C "$fixture" add scripts/hooks/failing-gate.sh
  git -C "$fixture" commit -qm "test: add failing gate"
  set +e
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/failing-gate.sh)"
  code=$?
  set -e
  assert_eq integrity-gate-failure-exit 7 "$code"
  assert_eq integrity-gate-failure-status gate_failed \
    "$(printf '%s' "$output" | json_field status)"
  assert_single_json "$output"
  assert_empty integrity-gate-failure-receipt "$(current_receipt "$fixture")"
  echo "PASS integrity-gate-failure"

  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  printf '#!/usr/bin/env bash\nkill -TERM "$PPID"\nexit 143\n' >"$fixture/scripts/hooks/signal-gate.sh"
  chmod +x "$fixture/scripts/hooks/signal-gate.sh"
  git -C "$fixture" add scripts/hooks/signal-gate.sh
  git -C "$fixture" commit -qm "test: add signal gate"
  set +e
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/signal-gate.sh)"
  code=$?
  set -e
  assert_ne integrity-signal-exit 0 "$code"
  assert_empty integrity-signal-receipt "$(current_receipt "$fixture")"
  echo "PASS integrity-signal"

  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  set +e
  (cd "$fixture" && CSA_QUALITY_GATE_TEST_FAULT=crash-before-publish \
    "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter") >/dev/null 2>&1
  code=$?
  set -e
  assert_ne integrity-crash-before-rename-exit 0 "$code"
  assert_empty integrity-crash-before-rename-receipt "$(current_receipt "$fixture")"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq integrity-crash-before-rename-recovery-status executed \
    "$(printf '%s' "$output" | json_field status)"
  echo "PASS integrity-crash-before-rename"

  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  receipt_dir="${fixture}/.csa/state/quality-gate-receipts"
  mkfifo "$fixture/.csa/state/ready" "$fixture/.csa/state/release"
  exec 7<>"$fixture/.csa/state/ready"
  exec 8<>"$fixture/.csa/state/release"
  printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>"$1"\nprintf "ready\\n" >"$2"\nread -r _ <"$3"\n' >"$fixture/scripts/hooks/blocking-gate.sh"
  chmod +x "$fixture/scripts/hooks/blocking-gate.sh"
  git -C "$fixture" add scripts/hooks/blocking-gate.sh
  git -C "$fixture" commit -qm "test: add blocking gate"
  (cd "$fixture" && "$runner" -- scripts/hooks/blocking-gate.sh "$counter" \
    .csa/state/ready .csa/state/release >.csa/state/writer-one.json) &
  local writer_one=$!
  if ! timeout 5 bash -c 'read -r _ <&7' 7<&7; then
    kill "$writer_one" 2>/dev/null || true
    wait "$writer_one" 2>/dev/null || true
    echo "timed out waiting for the first fixture writer" >&2
    return 1
  fi
  (cd "$fixture" && "$runner" -- scripts/hooks/blocking-gate.sh "$counter" \
    .csa/state/ready .csa/state/release >.csa/state/writer-two.json) &
  local writer_two=$!
  printf 'release\n' >&8
  wait_for_pid_bounded "$writer_one"
  wait_for_pid_bounded "$writer_two"
  exec 7>&- 8>&-
  assert_eq integrity-concurrency-initial-gate-runs 1 "$(wc -c <"$counter")"
  assert_eq integrity-concurrency-receipt-count 1 \
    "$(find "$receipt_dir" -maxdepth 1 -type f -name '*.json' | wc -l)"
  assert_single_json "$(cat "$fixture/.csa/state/writer-one.json")"
  assert_single_json "$(cat "$fixture/.csa/state/writer-two.json")"
  local writers=()
  for _ in 1 2 3 4 5 6; do
    (cd "$fixture" && "$runner" -- scripts/hooks/blocking-gate.sh "$counter" \
      .csa/state/ready .csa/state/release >/dev/null) &
    writers+=("$!")
  done
  local writer
  for writer in "${writers[@]}"; do
    wait_for_pid_bounded "$writer"
  done
  assert_eq integrity-concurrency-final-gate-runs 1 "$(wc -c <"$counter")"
  echo "PASS integrity-concurrency"
}

if [ "${BASH_SOURCE[0]}" != "$0" ]; then
  return 0
fi

case "$scenario" in
  exact-reuse) run_exact_reuse ;;
  path-toolchain-invalidation) run_path_toolchain_invalidation ;;
  mise-data-dir-invalidation) run_mise_data_dir_invalidation ;;
  invalidation-matrix) run_invalidation_matrix ;;
  integrity-concurrency) run_integrity_concurrency ;;
  all) run_exact_reuse; run_invalidation_matrix; run_integrity_concurrency ;;
  *) echo "unknown scenario: $scenario" >&2; exit 2 ;;
esac
