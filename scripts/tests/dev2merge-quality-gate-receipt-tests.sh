#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT

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
grep -q 'just quality-gates' <<<"$step_eleven"
grep -q 'elif \[ -f Cargo.toml \]' <<<"$step_eleven"

hook_receipt_field() {
  local field="$1"
  python3 -c '
import json,sys
field=sys.argv[1]
for line in sys.stdin:
    try: value=json.loads(line.strip())
    except json.JSONDecodeError: continue
    if "receipt_identity" in value:
        print(value[field]); break
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
    test "${2:-}" = sysroot
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
cat >"$fixture/justfile" <<EOF
quality-gates:
    MISE_DATA_DIR="$test_root/no-mise" PATH="\${CSA_TEST_RUSTC_DIR:?}:\$PATH" scripts/hooks/quality-gate-receipt.sh -- scripts/hooks/pre-push-quality-gates.sh

pre-push:
    CSA_TEST_RUSTC_DIR="$hook_rustc_dir" just quality-gates
EOF
cp "$repo_root/lefthook.yml" "$fixture/lefthook.yml"
git -C "$fixture" add Cargo.toml Cargo.lock justfile lefthook.yml rust-toolchain.toml scripts
git -C "$fixture" commit -qm "test: initialize dev2merge fixture"

producer_started_ns="$(date +%s%N)"
producer="$(cd "$fixture" && CSA_TEST_RUSTC_DIR="$direct_rustc_dir" just quality-gates)"
producer_elapsed_ms="$(( ($(date +%s%N) - producer_started_ns) / 1000000 ))"
producer_identity="$(printf '%s' "$producer" | python3 -c 'import json,sys; value=json.load(sys.stdin); assert value["status"] == "executed"; print(value["receipt_identity"])')"
(cd "$fixture" && scripts/hooks/review-check.sh)
consumer_started_ns="$(date +%s%N)"
consumer="$(cd "$fixture" && lefthook run pre-push 2>&1)"
consumer_elapsed_ms="$(( ($(date +%s%N) - consumer_started_ns) / 1000000 ))"
consumer_status="$(hook_receipt_field status <<<"$consumer")"
consumer_identity="$(hook_receipt_field receipt_identity <<<"$consumer")"

test "$consumer_status" = reused
test "$producer_identity" = "$consumer_identity"
test "$(wc -c <"$fixture/.csa/state/quality-counter")" -eq 1
test "$(wc -c <"$fixture/.csa/state/branch-protection-counter")" -eq 1
test "$(wc -c <"$fixture/.csa/state/version-check-counter")" -eq 1
test "$(wc -c <"$fixture/.csa/state/review-check-counter")" -eq 2

printf '# changed compiler bytes\n' >>"$toolchain_root/bin/rustc"
changed_consumer="$(cd "$fixture" && lefthook run pre-push 2>&1)"
changed_status="$(hook_receipt_field status <<<"$changed_consumer")"
changed_identity="$(hook_receipt_field receipt_identity <<<"$changed_consumer")"
test "$changed_status" = executed
test "$changed_identity" != "$producer_identity"
test "$(wc -c <"$fixture/.csa/state/quality-counter")" -eq 2

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
test "$provenance_status" = executed
test "$provenance_identity" != "$changed_identity"
test "$(wc -c <"$fixture/.csa/state/quality-counter")" -eq 3
test "$(wc -c <"$fixture/.csa/state/branch-protection-counter")" -eq 3
test "$(wc -c <"$fixture/.csa/state/version-check-counter")" -eq 3
test "$(wc -c <"$fixture/.csa/state/review-check-counter")" -eq 4
echo "PASS dev2merge-quality-gate-receipt identity=${producer_identity} changed_identity=${changed_identity} provenance_identity=${provenance_identity} quality_runs=3 executed_ms=${producer_elapsed_ms} reused_ms=${consumer_elapsed_ms}"
