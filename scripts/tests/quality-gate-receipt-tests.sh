#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
runner="${repo_root}/scripts/hooks/quality-gate-receipt.sh"
scenario="${1:-all}"

new_fixture() {
  local fixture
  fixture="$(mktemp -d)"
  git -C "$fixture" init -q
  git -C "$fixture" config user.name "Quality Gate Tests"
  git -C "$fixture" config user.email "quality-gate-tests@example.invalid"
  git -C "$fixture" remote add origin "https://example.invalid/quality-gate.git"
  mkdir -p "$fixture/scripts/hooks" "$fixture/.csa/state"
  cp "${repo_root}/scripts/rename-no-replace.py" "$fixture/scripts/rename-no-replace.py"
  printf '[workspace]\n' >"$fixture/Cargo.toml"
  printf '# lock\n' >"$fixture/Cargo.lock"
  printf '# weave\n' >"$fixture/weave.lock"
  printf 'quality-gates:\n    true\n' >"$fixture/justfile"
  printf 'pre-push: {}\n' >"$fixture/lefthook.yml"
  printf '#!/usr/bin/env bash\nset -euo pipefail\ncounter="$1"\nprintf "x" >>"$counter"\n' \
    >"$fixture/scripts/hooks/fake-quality-gate.sh"
  chmod +x "$fixture/scripts/hooks/fake-quality-gate.sh"
  git -C "$fixture" add Cargo.toml Cargo.lock weave.lock justfile lefthook.yml scripts
  git -C "$fixture" commit -qm "test: initialize fixture"
  printf '%s\n' "$fixture"
}

json_field() {
  python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$1"
}

run_exact_reuse() {
  local fixture counter first second first_status second_status first_identity second_identity
  fixture="$(new_fixture)"
  trap 'rm -rf -- "$fixture"' RETURN
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

case "$scenario" in
  exact-reuse) run_exact_reuse ;;
  all) run_exact_reuse ;;
  *) echo "unknown scenario: $scenario" >&2; exit 2 ;;
esac
