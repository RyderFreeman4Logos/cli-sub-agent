#!/usr/bin/env bash
set -euo pipefail
shopt -s inherit_errexit
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
export GIT_CONFIG_NOSYSTEM=1
# Direct receipt-state inspections import local modules, so this contract suite
# must not mutate the clean-worktree input by writing checkout bytecode.
export PYTHONDONTWRITEBYTECODE=1

repo_root="$(git rev-parse --show-toplevel)"
source "$repo_root/scripts/tests/quality-gate-test-assertions.sh"
source "$repo_root/scripts/tests/quality-gate-receipt-integrity-tests.sh"
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  receipt_contract_install_failure_trap quality-gate-receipt-tests.sh
fi
source_runner="${repo_root}/scripts/hooks/quality-gate-receipt.sh"
scenario="${1:-all}"
mkdir -p "$repo_root/drafts"
test_root="$(realpath -e "$(mktemp -d "$repo_root/drafts/quality-gate-receipts.XXXXXX")")"
owned_pids=()

register_child() {
  owned_pids+=("$1")
}

unregister_child() {
  local removed="$1" pid
  local -a remaining=()
  for pid in "${owned_pids[@]}"; do
    [ "$pid" = "$removed" ] || remaining+=("$pid")
  done
  owned_pids=("${remaining[@]}")
}

cleanup_receipt_tests() {
  local pid
  for pid in "${owned_pids[@]}"; do
    kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  rm -rf -- "$test_root"
}
trap cleanup_receipt_tests EXIT
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
  if [ "${CSA_QUALITY_GATE_TEST_SETUP_FAILURE:-}" = after-init ]; then
    return 73
  fi
  mkdir -p "$fixture/scripts/hooks" "$fixture/.csa/state" \
    "$fixture/target/quality-gate-test-state"
  printf '/.csa/state/\n/target/\n' >"$fixture/.gitignore"
  cp "${repo_root}/scripts/rename-no-replace.py" "$fixture/scripts/rename-no-replace.py"
  cp "${repo_root}/scripts/cargo-env-normalize.sh" "$fixture/scripts/cargo-env-normalize.sh"
  cp "${repo_root}/scripts/quality-gate-state.py" "$fixture/scripts/quality-gate-state.py"
  cp "${repo_root}/scripts/quality_gate_secure_state.py" \
    "$fixture/scripts/quality_gate_secure_state.py"
  cp "${repo_root}/scripts/quality_gate_provenance.py" \
    "$fixture/scripts/quality_gate_provenance.py"
  cp "${repo_root}/scripts/quality_gate_sandbox.py" \
    "$fixture/scripts/quality_gate_sandbox.py"
  cp "${repo_root}/scripts/quality_gate_host_attestation.py" \
    "$fixture/scripts/quality_gate_host_attestation.py"
  cp "${repo_root}/scripts/quality_gate_process.py" \
    "$fixture/scripts/quality_gate_process.py"
  cp "${repo_root}/scripts/quality_gate_environment.py" \
    "$fixture/scripts/quality_gate_environment.py"
  cp "${repo_root}/scripts/quality_gate_toolchain.py" \
    "$fixture/scripts/quality_gate_toolchain.py"
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
  git -C "$fixture" add .gitignore Cargo.toml Cargo.lock weave.lock justfile \
    lefthook.yml rust-toolchain.toml scripts
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
  counter="${fixture}/target/quality-gate-test-state/gate-counter"

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
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  (cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter") >/dev/null
  manifest="$(receipt_manifest "$fixture")"
  for key in \
    repository_identity checkout_identity head_oid tree_oid index_oid index_tree_oid \
    index_clean tracked_worktree_clean untracked_worktree_digest \
    cargo_lock_sha256 weave_lock_sha256 rust_toolchain_sha256 rust_toolchain_file_sha256 \
    rust_toolchain_launcher_authority_sha256 \
    rust_toolchain_launcher_invocation_sha256 \
    rust_toolchain_semantic_projection \
    target_provenance_sha256 feature_matrix_sha256 environment_sha256 \
    cargo_config_sha256 dotenv_sha256 normalizer_sha256 tool_provenance_sha256 \
    justfile_sha256 lefthook_sha256 gate_script_sha256 recipe_sha256 \
    implementation_sha256 quality_gate_state_helper_sha256 \
    quality_gate_secure_state_sha256 \
    quality_gate_provenance_sha256 quality_gate_sandbox_sha256 \
    quality_gate_host_attestation_sha256 \
    quality_gate_toolchain_sha256 \
    quality_gate_process_sha256 quality_gate_environment_sha256 \
    quality_gate_entrypoint_sha256 quality_gate_live_sha256 \
    source_host_sha256 source_snapshot_sha256 sandbox_version \
    schema_version implementation_version; do
    if ! grep -q "^${key}=" <<<"$manifest"; then
      _receipt_test_fail "manifest-contract-${key}" field-present field-missing
      return 1
    fi
  done
}

invoke_identity() {
  local fixture="$1" counter="$2"
  local runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
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
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
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

assert_sanitized_environment() {
  local name="$1" first_env="$2" second_env="$3"
  local fixture counter first second
  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  first="$(invoke_identity "$fixture" "$counter" "$first_env")"
  second="$(invoke_identity "$fixture" "$counter" "$second_env")"
  assert_eq "sanitized-${name}-identity" "$first" "$second"
  assert_eq "sanitized-${name}-gate-runs" 1 "$(wc -c <"$counter")"
  echo "PASS sanitized-$name"
}

run_path_toolchain_canonicalization() {
  local fixture counter first second runs toolchain toolchain_root real_sysroot
  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  real_sysroot="$(rustc --print sysroot)"
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
    printf '%s\\n' "$real_sysroot"
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
  assert_eq canonicalized-pinned-toolchain-identity "$first" "$second"
  runs="$(wc -c <"$counter")"
  assert_eq canonicalized-pinned-toolchain-gate-runs 1 "$runs"
  echo "PASS canonicalized-pinned-toolchain"
}

run_mise_data_dir_invalidation() {
  local fixture counter first second
  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  first="$(invoke_identity "$fixture" "$counter" \
    "MISE_DATA_DIR=${test_root}/mise-a")"
  second="$(invoke_identity "$fixture" "$counter" \
    "MISE_DATA_DIR=${test_root}/mise-b")"
  assert_eq sanitized-mise-data-dir-identity "$first" "$second"
  assert_eq sanitized-mise-data-dir-gate-runs 1 "$(wc -c <"$counter")"
  echo "PASS sanitized-mise-data-dir"
}

run_fixture_and_interface_contracts() {
  local output code fake_bin exports
  set +e
  output="$(CSA_QUALITY_GATE_TEST_SETUP_FAILURE=after-init new_fixture 2>&1)"
  code=$?
  set -e
  assert_eq fixture-setup-failure-exit 73 "$code"
  assert_not_matches fixture-setup-failure-no-pass '^PASS ' "$output"

  fake_bin="$test_root/fake-grep"
  mkdir -p "$fake_bin"
  printf '#!/usr/bin/env bash\nexit 2\n' >"$fake_bin/grep"
  chmod +x "$fake_bin/grep"
  set +e
  output="$(PATH="$fake_bin:$PATH" bash -c \
    'source "$1"; assert_not_matches matcher-error needle haystack' _ \
    "$repo_root/scripts/tests/quality-gate-test-assertions.sh" 2>&1)"
  code=$?
  set -e
  assert_eq interface-matcher-error-exit 1 "$code"
  assert_contains interface-matcher-error-diagnostic matcher-error "$output"

  output="$(_receipt_test_evidence 'PASSWORD=do-not-print')"
  assert_contains interface-secret-marker-redacted sha256: "$output"
  assert_not_matches interface-secret-marker-absent 'PASSWORD|do-not-print' "$output"

  exports="$(python3 - "$repo_root/scripts/quality_gate_provenance.py" \
    "$repo_root/scripts/quality_gate_secure_state.py" <<'PY'
import ast
import sys

for filename in sys.argv[1:]:
    tree = ast.parse(open(filename, encoding="utf-8").read())
    assignments = [
        node
        for node in tree.body
        if isinstance(node, ast.Assign)
        and any(
            isinstance(target, ast.Name) and target.id == "__all__"
            for target in node.targets
        )
    ]
    if len(assignments) != 1:
        raise SystemExit(1)
    print(filename.rsplit("/", 1)[-1])
PY
)"
  assert_contains interface-provenance-all quality_gate_provenance.py "$exports"
  assert_contains interface-secure-state-all quality_gate_secure_state.py "$exports"

  assert_path_absent interface-no-bytecode-cache-before \
    "$repo_root/scripts/__pycache__"
  set +e
  output="$(python3 "$repo_root/scripts/quality-gate-state.py" collect \
    --repo "$test_root/missing-PASSWORD-evidence" -- /bin/true 2>&1)"
  code=$?
  set -e
  assert_eq interface-bounded-cli-error-exit 2 "$code"
  assert_eq interface-bounded-cli-error-lines 1 "$(wc -l <<<"$output")"
  assert_not_matches interface-bounded-cli-error-sanitized \
    'Traceback|PASSWORD|missing-|/home/|/tmp/' "$output"
  assert_path_absent interface-no-bytecode-cache-after \
    "$repo_root/scripts/__pycache__"
  echo "PASS fixture-and-interface-contracts"
}

run_invalidation_matrix() {
  assert_manifest_contract
  assert_invalidation head 'printf "head\n" >"$fixture/head"; git -C "$fixture" add head; git -C "$fixture" commit -qm "test: change head"'
  assert_invalidation index 'printf "index\n" >"$fixture/index"; git -C "$fixture" add index'
  assert_invalidation tracked-worktree 'printf "dirty\n" >>"$fixture/Cargo.toml"'
  assert_invalidation untracked-worktree 'printf "untracked\n" >"$fixture/untracked"'
  assert_invalidation repository 'mv "$fixture/.git" "$fixture/.git-store"; printf "gitdir: .git-store\\n" >"$fixture/.git"'
  assert_invalidation checkout 'moved="${fixture}.moved"; mv "$fixture" "$moved"; fixture="$moved"; counter="${fixture}/target/quality-gate-test-state/gate-counter"'
  assert_invalidation cargo-lock 'printf "changed\n" >>"$fixture/Cargo.lock"'
  assert_invalidation weave-lock 'printf "changed\n" >>"$fixture/weave.lock"'
  assert_invalidation rust-toolchain-file \
    'printf "# changed toolchain contract\n" >>"$fixture/rust-toolchain.toml"'
  run_path_toolchain_canonicalization
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
  assert_sanitized_environment cargo-deny-disable-fetch \
    'CARGO_DENY_DISABLE_FETCH=0' 'CARGO_DENY_DISABLE_FETCH=1'
  assert_sanitized_environment cargo-deny-offline \
    'CARGO_DENY_OFFLINE=0' 'CARGO_DENY_OFFLINE=1'
  assert_sanitized_environment cargo-net-offline \
    'CARGO_NET_OFFLINE=false' 'CARGO_NET_OFFLINE=true'
  assert_invalidation nextest-profile ':' \
    'NEXTEST_PROFILE=default' 'NEXTEST_PROFILE=ci'
  assert_invalidation nextest-test-threads ':' \
    'NEXTEST_TEST_THREADS=1' 'NEXTEST_TEST_THREADS=2'
  assert_invalidation rustc-bootstrap ':' \
    'RUSTC_BOOTSTRAP=0' 'RUSTC_BOOTSTRAP=1'
  assert_invalidation cargo-home ':' \
    "CARGO_HOME=${test_root}/cargo-home-a" "CARGO_HOME=${test_root}/cargo-home-b"
  assert_sanitized_environment cargo-target-dir \
    "CARGO_TARGET_DIR=${test_root}/raw-target-a" \
    "CARGO_TARGET_DIR=${test_root}/raw-target-b"
  assert_invalidation target ':' '' 'CARGO_BUILD_TARGET=other-linux-target'
  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
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
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  before="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter" | json_field receipt_identity)"
  printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf "x" >>"$1"\nprintf ready >"$2"\nwhile [ ! -e "$3" ]; do sleep 0.02; done\n' >"$fixture/scripts/hooks/drift-gate.sh"
  chmod +x "$fixture/scripts/hooks/drift-gate.sh"
  git -C "$fixture" add scripts/hooks/drift-gate.sh
  git -C "$fixture" commit -qm "test: add drift gate"
  (
    cd "$fixture"
    exec "$runner" -- scripts/hooks/drift-gate.sh "$counter" \
      target/quality-gate-test-state/drift-ready \
      target/quality-gate-test-state/drift-release \
      >target/quality-gate-test-state/drift-output.json
  ) &
  local drift_writer=$!
  register_child "$drift_writer"
  if ! timeout 5 bash -c 'until [ -e "$1" ]; do sleep 0.02; done' _ \
    "$fixture/target/quality-gate-test-state/drift-ready"; then
    kill -KILL "$drift_writer" 2>/dev/null || true
    wait "$drift_writer" 2>/dev/null || true
    unregister_child "$drift_writer"
    _receipt_test_fail invalidation-input-drift-ready ready timed-out
    return 1
  fi
  printf 'drift\n' >>"$fixture/Cargo.toml"
  touch "$fixture/target/quality-gate-test-state/drift-release"
  wait_for_pid_bounded "$drift_writer"
  after="$(<"$fixture/target/quality-gate-test-state/drift-output.json")"
  local after_identity
  after_identity="$(printf '%s' "$after" | json_field receipt_identity)"
  assert_eq invalidation-input-drift-reason input_drift \
    "$(printf '%s' "$after" | json_field rejection_reason)"
  assert_path_absent invalidation-input-drift-no-receipt \
    "$fixture/.csa/state/quality-gate-receipts/${after_identity}.json"
  assert_ne invalidation-input-drift-identity "$before" "$after_identity"
  echo "PASS invalidation-input-drift"
}

if [ "${BASH_SOURCE[0]}" != "$0" ]; then
  return 0
fi

case "$scenario" in
  exact-reuse)
    receipt_contract_set_case exact-reuse
    run_exact_reuse
    ;;
  fixture-interface)
    receipt_contract_set_case fixture-interface
    run_fixture_and_interface_contracts
    ;;
  path-toolchain-canonicalization)
    receipt_contract_set_case path-toolchain-canonicalization
    run_path_toolchain_canonicalization
    ;;
  mise-data-dir-invalidation)
    receipt_contract_set_case mise-data-dir-invalidation
    run_mise_data_dir_invalidation
    ;;
  invalidation-matrix)
    receipt_contract_set_case invalidation-matrix
    run_invalidation_matrix
    ;;
  integrity-concurrency)
    receipt_contract_set_case integrity-concurrency
    run_integrity_concurrency
    ;;
  all)
    receipt_contract_set_case fixture-interface
    run_fixture_and_interface_contracts
    receipt_contract_set_case exact-reuse
    run_exact_reuse
    receipt_contract_set_case invalidation-matrix
    run_invalidation_matrix
    receipt_contract_set_case integrity-concurrency
    run_integrity_concurrency
    ;;
  *) echo "unknown scenario: $scenario" >&2; exit 2 ;;
esac
