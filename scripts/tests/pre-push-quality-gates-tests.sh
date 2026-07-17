#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
source "$repo_root/scripts/tests/quality-gate-test-assertions.sh"
scenario="${1:-receipt-reuse-with-hard-gates}"
test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT

require_source_contract() {
  local summary quality_recipe pre_push_recipe pre_push_source contract_source
  local lefthook_source count suite
  summary="$(just --no-dotenv --summary)"
  quality_recipe="$(just --no-dotenv --show quality-gates)"
  pre_push_recipe="$(just --no-dotenv --show pre-push)"
  pre_push_source="$(<scripts/hooks/pre-push-quality-gates.sh)"
  contract_source="$(<scripts/hooks/quality-gate-contract-tests.sh)"
  lefthook_source="$(<lefthook.yml)"
  assert_contains source-contract-quality-recipe-listed quality-gates "$summary"
  assert_contains source-contract-quality-recipe-entrypoint \
    scripts/hooks/quality-gates.sh "$quality_recipe"
  assert_contains source-contract-pre-push-alias 'pre-push: quality-gates' "$pre_push_recipe"
  count="$(grep -c 'scripts/hooks/quality-gate-contract-tests.sh' \
    <<<"$pre_push_source" || true)"
  assert_eq source-contract-authoritative-stage-count 1 "$count"
  for suite in \
    quality-gate-receipt-tests.sh \
    quality-gate-receipt-hostile-tests.sh \
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
    printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>.csa/state/%s-counter\nif [ -e .csa/state/fail-%s ]; then printf "fixture %s gate forced failure\\n" >&2; exit 1; fi\n' \
      "$gate" "$gate" "$gate" >"$fixture/scripts/hooks/${gate}.sh"
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
  local -a receipts
  fixture="$(new_fixture)"
  (cd "$fixture" && lefthook run pre-push >/dev/null)
  (cd "$fixture" && lefthook run pre-push >/dev/null)
  quality="$(wc -c <"$fixture/.csa/state/quality-counter")"
  assert_eq receipt-reuse-quality-runs 1 "$quality"
  for gate in branch-protection version-check review-check; do
    assert_eq "receipt-reuse-${gate}-runs" 2 \
      "$(wc -c <"$fixture/.csa/state/${gate}-counter")"
  done
  mapfile -t receipts < <(
    find "$fixture/.csa/state/quality-gate-receipts" -maxdepth 1 -type f -name '*.json'
  )
  assert_eq receipt-reuse-receipt-count 1 "${#receipts[@]}"
  identity="$(basename "${receipts[0]}" .json)"
  assert_nonempty receipt-reuse-identity "$identity"

  for gate in branch-protection version-check review-check; do
    before="$(wc -c <"$fixture/.csa/state/${gate}-counter")"
    touch "$fixture/.csa/state/fail-${gate}"
    set +e
    (cd "$fixture" && lefthook run pre-push >/dev/null 2>&1)
    code=$?
    set -e
    rm "$fixture/.csa/state/fail-${gate}"
    assert_ne "hard-gate-${gate}-failure-exit" 0 "$code"
    assert_eq "hard-gate-${gate}-quality-runs" 1 \
      "$(wc -c <"$fixture/.csa/state/quality-counter")"
    assert_eq "hard-gate-${gate}-runs" "$((before + 1))" \
      "$(wc -c <"$fixture/.csa/state/${gate}-counter")"
  done
  echo "PASS receipt-reuse-with-hard-gates identity=${identity}"
}

case "$scenario" in
  receipt-reuse-with-hard-gates) run_contract ;;
  *) echo "unknown scenario: $scenario" >&2; exit 2 ;;
esac
