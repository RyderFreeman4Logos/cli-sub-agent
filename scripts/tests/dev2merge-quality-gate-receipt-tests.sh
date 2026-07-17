#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
source "$repo_root/scripts/tests/quality-gate-test-assertions.sh"
test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT
python_executable="$(python3 -c 'import os,sys; print(os.path.realpath(sys.executable))')"
assert_executable dev2merge-fixture-python-launcher "$python_executable"
fixture_base_path="$(python3 -c 'import os; print(os.environ["PATH"])')"
assert_nonempty dev2merge-fixture-base-path "$fixture_base_path"

step_eleven="$(python3 - "$repo_root/patterns/dev2merge/workflow.toml" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as source:
    workflow = tomllib.load(source)
for step in workflow["workflow"]["steps"]:
    if step["title"] == "Self-Review Gate":
        print(step["prompt"])
        break
PY
)"
assert_contains dev2merge-workflow-quality-gate 'just quality-gates' "$step_eleven"
assert_contains dev2merge-workflow-cargo-fallback 'elif [ -f Cargo.toml ]' "$step_eleven"

hook_receipt_field() {
  local field="$1"
  python3 -c '
import json,sys
field=sys.argv[1]
for line in sys.stdin:
    try: value=json.loads(line.strip())
    except json.JSONDecodeError: continue
    if "receipt_identity" in value and field in value:
        print(value[field])
        raise SystemExit(0)
print(f"FAIL hook-receipt-{field} expected=field-present actual=field-missing", file=sys.stderr)
raise SystemExit(1)
' "$field"
}

fixture="$test_root/repo"
toolchain_root="$test_root/toolchain"
direct_rustc_dir="$test_root/direct-rustc"
hook_rustc_dir="$test_root/hook-rustc"
mkdir -p "$toolchain_root/bin" "$direct_rustc_dir" "$hook_rustc_dir"
cat >"$toolchain_root/bin/rustc" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
toolchain_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
case "${1:-}" in
  -vV)
    cat "$toolchain_root/provenance"
    ;;
  --print)
    if [ "${2:-}" != sysroot ]; then
      printf 'fixture rustc expected --print sysroot, got: %s\n' "${2:-unset}" >&2
      exit 64
    fi
    printf '%s\n' "$toolchain_root"
    ;;
  *)
    echo "unsupported fixture rustc arguments: $*" >&2
    exit 64
    ;;
esac
EOF
cat >"$toolchain_root/provenance" <<'EOF'
rustc 1.99.0 (111111111 2099-01-01)
binary: rustc
commit-hash: 1111111111111111111111111111111111111111
commit-date: 2099-01-01
host: x86_64-unknown-linux-gnu
release: 1.99.0
LLVM version: 99.0.0
EOF
for entry in direct hook; do
  printf '#!/usr/bin/env bash\n# %s launcher\nexec %q "$@"\n' \
    "$entry" "$toolchain_root/bin/rustc" >"$test_root/${entry}-rustc/rustc"
done
chmod +x "$toolchain_root/bin/rustc" "$direct_rustc_dir/rustc" "$hook_rustc_dir/rustc"
ln -s "$python_executable" "$direct_rustc_dir/python3"
ln -s "$python_executable" "$hook_rustc_dir/python3"

mkdir -p "$fixture/scripts/hooks" "$fixture/scripts" "$fixture/.csa/state"
git -C "$fixture" init -q
git -C "$fixture" config user.name "Dev2merge Tests"
git -C "$fixture" config user.email "dev2merge-tests@example.invalid"
git -C "$fixture" remote add origin https://example.invalid/dev2merge.git
cp "$repo_root/scripts/hooks/quality-gate-receipt.sh" "$fixture/scripts/hooks/"
cp "$repo_root/scripts/cargo-env-normalize.sh" "$fixture/scripts/"
cp "$repo_root/scripts/quality-gate-state.py" "$fixture/scripts/"
cp "$repo_root/scripts/quality_gate_secure_state.py" "$fixture/scripts/"
cp "$repo_root/scripts/quality_gate_provenance.py" "$fixture/scripts/"
cp "$repo_root/scripts/rename-no-replace.py" "$fixture/scripts/"
cp "$repo_root/rust-toolchain.toml" "$fixture/"
printf '[workspace]\n' >"$fixture/Cargo.toml"
printf '# lock\n' >"$fixture/Cargo.lock"
printf '# weave\n' >"$fixture/weave.lock"
printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>.csa/state/quality-counter\n' \
  >"$fixture/scripts/hooks/pre-push-quality-gates.sh"
for gate in branch-protection version-check review-check; do
  printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>.csa/state/%s-counter\n' "$gate" \
    >"$fixture/scripts/hooks/${gate}.sh"
done
chmod +x "$fixture/scripts/hooks/"*.sh
# Freeze host tool resolution so direct and hook entrypoints differ only by the
# synthetic rustc launcher; normalized compiler changes remain observable.
cat >"$fixture/justfile" <<EOF
quality-gates:
    @env -u CARGO_HOME -u RUSTUP_HOME MISE_DATA_DIR="$test_root/no-mise" PATH="\${CSA_TEST_RUSTC_DIR:?}:$fixture_base_path" scripts/hooks/quality-gate-receipt.sh -- scripts/hooks/pre-push-quality-gates.sh

pre-push:
    CSA_TEST_RUSTC_DIR="$hook_rustc_dir" just quality-gates
EOF
cp "$repo_root/lefthook.yml" "$fixture/lefthook.yml"
git -C "$fixture" add Cargo.toml Cargo.lock justfile lefthook.yml rust-toolchain.toml scripts
git -C "$fixture" commit -qm "test: initialize dev2merge fixture"

producer_started_ns="$(date +%s%N)"
producer="$(cd "$fixture" && CSA_TEST_RUSTC_DIR="$direct_rustc_dir" just quality-gates)"
producer_elapsed_ms="$(( ($(date +%s%N) - producer_started_ns) / 1000000 ))"
producer_status="$(hook_receipt_field status <<<"$producer")"
producer_identity="$(hook_receipt_field receipt_identity <<<"$producer")"
assert_eq dev2merge-producer-status executed "$producer_status"
(cd "$fixture" && scripts/hooks/review-check.sh)
consumer_started_ns="$(date +%s%N)"
consumer="$(cd "$fixture" && lefthook run pre-push 2>&1)"
consumer_elapsed_ms="$(( ($(date +%s%N) - consumer_started_ns) / 1000000 ))"
consumer_status="$(hook_receipt_field status <<<"$consumer")"
consumer_identity="$(hook_receipt_field receipt_identity <<<"$consumer")"

assert_eq dev2merge-consumer-status reused "$consumer_status"
assert_eq dev2merge-consumer-identity "$producer_identity" "$consumer_identity"
assert_eq dev2merge-reuse-quality-runs 1 "$(wc -c <"$fixture/.csa/state/quality-counter")"
assert_eq dev2merge-reuse-branch-protection-runs 1 \
  "$(wc -c <"$fixture/.csa/state/branch-protection-counter")"
assert_eq dev2merge-reuse-version-check-runs 1 \
  "$(wc -c <"$fixture/.csa/state/version-check-counter")"
assert_eq dev2merge-reuse-review-check-runs 2 \
  "$(wc -c <"$fixture/.csa/state/review-check-counter")"

printf '# changed compiler bytes\n' >>"$toolchain_root/bin/rustc"
changed_consumer="$(cd "$fixture" && lefthook run pre-push 2>&1)"
changed_status="$(hook_receipt_field status <<<"$changed_consumer")"
changed_identity="$(hook_receipt_field receipt_identity <<<"$changed_consumer")"
assert_eq dev2merge-compiler-bytes-status executed "$changed_status"
assert_ne dev2merge-compiler-bytes-identity "$producer_identity" "$changed_identity"
assert_eq dev2merge-compiler-bytes-quality-runs 2 \
  "$(wc -c <"$fixture/.csa/state/quality-counter")"

cat >"$toolchain_root/provenance" <<'EOF'
rustc 1.99.1 (222222222 2099-01-02)
binary: rustc
commit-hash: 2222222222222222222222222222222222222222
commit-date: 2099-01-02
host: x86_64-unknown-linux-gnu
release: 1.99.1
LLVM version: 99.0.0
EOF
provenance_consumer="$(cd "$fixture" && lefthook run pre-push 2>&1)"
provenance_status="$(hook_receipt_field status <<<"$provenance_consumer")"
provenance_identity="$(hook_receipt_field receipt_identity <<<"$provenance_consumer")"
assert_eq dev2merge-compiler-provenance-status executed "$provenance_status"
assert_ne dev2merge-compiler-provenance-identity "$changed_identity" "$provenance_identity"
assert_eq dev2merge-compiler-provenance-quality-runs 3 \
  "$(wc -c <"$fixture/.csa/state/quality-counter")"
assert_eq dev2merge-final-branch-protection-runs 3 \
  "$(wc -c <"$fixture/.csa/state/branch-protection-counter")"
assert_eq dev2merge-final-version-check-runs 3 \
  "$(wc -c <"$fixture/.csa/state/version-check-counter")"
assert_eq dev2merge-final-review-check-runs 4 \
  "$(wc -c <"$fixture/.csa/state/review-check-counter")"
echo "PASS dev2merge-quality-gate-receipt identity=${producer_identity} changed_identity=${changed_identity} provenance_identity=${provenance_identity} quality_runs=3 executed_ms=${producer_elapsed_ms} reused_ms=${consumer_elapsed_ms}"
