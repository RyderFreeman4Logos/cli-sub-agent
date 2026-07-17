#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
scenario="${1:-receipt-reuse-with-hard-gates}"
test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT

require_source_contract() {
  just --summary | tr ' ' '\n' | grep -qx quality-gates
  just --show quality-gates | grep -q 'scripts/hooks/quality-gates.sh'
  just --show pre-push | grep -q 'pre-push: quality-gates'
  test "$(grep -c 'scripts/hooks/quality-gate-contract-tests.sh' \
    scripts/hooks/pre-push-quality-gates.sh)" -eq 1
  local suite
  for suite in \
    quality-gate-receipt-tests.sh \
    quality-gate-receipt-hostile-tests.sh \
    pre-push-quality-gates-tests.sh \
    dev2merge-quality-gate-receipt-tests.sh; do
    test "$(grep -c "scripts/tests/${suite}" \
      scripts/hooks/quality-gate-contract-tests.sh)" -eq 1
  done
  grep -q 'run: just pre-push' lefthook.yml
  grep -q 'run: scripts/hooks/branch-protection.sh' lefthook.yml
  grep -q 'run: scripts/hooks/version-check.sh' lefthook.yml
  grep -q 'run: scripts/hooks/review-check.sh' lefthook.yml
}

new_fixture() {
  local fixture="$test_root/repo"
  mkdir -p "$fixture/scripts/hooks" "$fixture/scripts" "$fixture/.csa/state"
  git -C "$fixture" init -q
  git -C "$fixture" config user.name "Pre-push Tests"
  git -C "$fixture" config user.email "pre-push-tests@example.invalid"
  git -C "$fixture" remote add origin https://example.invalid/pre-push.git
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
  local gate
  for gate in branch-protection version-check review-check; do
    printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>.csa/state/%s-counter\n[ ! -e .csa/state/fail-%s ]\n' "$gate" "$gate" \
      >"$fixture/scripts/hooks/${gate}.sh"
  done
  chmod +x "$fixture/scripts/hooks/"*.sh
  cat >"$fixture/justfile" <<'EOF'
quality-gates:
    scripts/hooks/quality-gate-receipt.sh -- scripts/hooks/pre-push-quality-gates.sh

pre-push:
    just quality-gates
EOF
  cp "$repo_root/lefthook.yml" "$fixture/lefthook.yml"
  git -C "$fixture" add Cargo.toml Cargo.lock justfile lefthook.yml rust-toolchain.toml scripts
  git -C "$fixture" commit -qm "test: initialize pre-push fixture"
  printf '%s\n' "$fixture"
}

run_contract() {
  require_source_contract
  local fixture quality identity gate before code
  fixture="$(new_fixture)"
  (cd "$fixture" && lefthook run pre-push >/dev/null)
  (cd "$fixture" && lefthook run pre-push >/dev/null)
  quality="$(wc -c <"$fixture/.csa/state/quality-counter")"
  test "$quality" -eq 1
  for gate in branch-protection version-check review-check; do
    test "$(wc -c <"$fixture/.csa/state/${gate}-counter")" -eq 2
  done
  identity="$(basename "$(find "$fixture/.csa/state/quality-gate-receipts" -name '*.json')" .json)"
  test -n "$identity"

  for gate in branch-protection version-check review-check; do
    before="$(wc -c <"$fixture/.csa/state/${gate}-counter")"
    touch "$fixture/.csa/state/fail-${gate}"
    set +e
    (cd "$fixture" && lefthook run pre-push >/dev/null 2>&1)
    code=$?
    set -e
    rm "$fixture/.csa/state/fail-${gate}"
    test "$code" -ne 0
    test "$(wc -c <"$fixture/.csa/state/quality-counter")" -eq 1
    test "$(wc -c <"$fixture/.csa/state/${gate}-counter")" -eq $((before + 1))
  done
  echo "PASS receipt-reuse-with-hard-gates identity=${identity}"
}

case "$scenario" in
  receipt-reuse-with-hard-gates) run_contract ;;
  *) echo "unknown scenario: $scenario" >&2; exit 2 ;;
esac
