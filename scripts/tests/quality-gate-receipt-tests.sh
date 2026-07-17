#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
source_runner="${repo_root}/scripts/hooks/quality-gate-receipt.sh"
scenario="${1:-all}"
test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT

new_fixture() {
  local fixture
  fixture="$(mktemp -d "${test_root}/fixture.XXXXXX")"
  git -C "$fixture" init -q
  git -C "$fixture" config user.name "Quality Gate Tests"
  git -C "$fixture" config user.email "quality-gate-tests@example.invalid"
  git -C "$fixture" remote add origin "https://example.invalid/quality-gate.git"
  mkdir -p "$fixture/scripts/hooks" "$fixture/.csa/state"
  cp "${repo_root}/scripts/rename-no-replace.py" "$fixture/scripts/rename-no-replace.py"
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
  git -C "$fixture" add Cargo.toml Cargo.lock weave.lock justfile lefthook.yml rust-toolchain.toml scripts
  git -C "$fixture" commit -qm "test: initialize fixture"
  printf '%s\n' "$fixture"
}

json_field() {
  python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$1"
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

  test "$first_status" = "executed"
  test "$second_status" = "reused"
  test "$first_identity" = "$second_identity"
  test "$(wc -c <"$counter")" -eq 1
  test "$(find "$fixture/.csa/state/quality-gate-receipts" -type f -name '*.json' | wc -l)" -eq 1
  test -z "$(git -C "$fixture" status --short)"
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
    cargo_lock_sha256 weave_lock_sha256 rust_toolchain_sha256 \
    target_provenance_sha256 feature_matrix_sha256 environment_sha256 \
    justfile_sha256 lefthook_sha256 gate_script_sha256 recipe_sha256 \
    implementation_sha256 schema_version implementation_version; do
    grep -q "^${key}=" <<<"$manifest" || {
      echo "missing manifest dimension: $key" >&2
      return 1
    }
  done
}

invoke_identity() {
  local fixture="$1" counter="$2" runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  shift 2
  (cd "$fixture" && env "$@" "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter") |
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
  test "$first" != "$second"
  test "$(wc -c <"$counter")" -eq 2
  echo "PASS invalidation-$name"
}

run_invalidation_matrix() {
  assert_manifest_contract
  assert_invalidation head 'printf "head\n" >"$fixture/head"; git -C "$fixture" add head; git -C "$fixture" commit -qm "test: change head"'
  assert_invalidation index 'printf "index\n" >"$fixture/index"; git -C "$fixture" add index'
  assert_invalidation tracked-worktree 'printf "dirty\n" >>"$fixture/Cargo.toml"'
  assert_invalidation untracked-worktree 'printf "untracked\n" >"$fixture/untracked"'
  assert_invalidation repository 'git -C "$fixture" remote set-url origin https://example.invalid/other.git'
  assert_invalidation checkout 'moved="${fixture}.moved"; mv "$fixture" "$moved"; fixture="$moved"; counter="${fixture}/.csa/state/gate-counter"'
  assert_invalidation cargo-lock 'printf "changed\n" >>"$fixture/Cargo.lock"'
  assert_invalidation weave-lock 'printf "changed\n" >>"$fixture/weave.lock"'
  local fixture counter first second toolchain
  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  for toolchain in toolchain-a toolchain-b; do
    mkdir -p "$fixture/$toolchain"
    printf '#!/usr/bin/env bash\nprintf "rustc 1.99.0\\nbinary: %s\\nhost: x86_64-unknown-linux-gnu\\n"\n' "$toolchain" \
      >"$fixture/$toolchain/rustc"
    chmod +x "$fixture/$toolchain/rustc"
  done
  first="$(invoke_identity "$fixture" "$counter" "PATH=${fixture}/toolchain-a:${PATH}")"
  second="$(invoke_identity "$fixture" "$counter" "PATH=${fixture}/toolchain-b:${PATH}")"
  test "$first" != "$second"
  test "$(wc -c <"$counter")" -eq 2
  echo "PASS invalidation-toolchain"
  assert_invalidation target ':' '' 'CARGO_BUILD_TARGET=other-linux-target'
  assert_invalidation feature-matrix ':' 'CSA_QUALITY_GATE_FEATURE_MATRIX=default' 'CSA_QUALITY_GATE_FEATURE_MATRIX=all-features'
  assert_invalidation environment ':' 'RUSTFLAGS=-Copt-level=1' 'RUSTFLAGS=-Copt-level=2'
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
  test "$(printf '%s' "$after" | json_field rejection_reason)" = input_drift
  test ! -e "$fixture/.csa/state/quality-gate-receipts/$(printf '%s' "$after" | json_field receipt_identity).json"
  test "$before" != "$(printf '%s' "$after" | json_field receipt_identity)"
  echo "PASS invalidation-input-drift"
}

case "$scenario" in
  exact-reuse) run_exact_reuse ;;
  invalidation-matrix) run_invalidation_matrix ;;
  all) run_exact_reuse; run_invalidation_matrix ;;
  *) echo "unknown scenario: $scenario" >&2; exit 2 ;;
esac
