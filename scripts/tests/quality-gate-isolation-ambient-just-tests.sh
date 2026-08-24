# shellcheck shell=bash
# Ambient Just shim isolation contract.

run_nested_just_ambient_input_isolation() {
  local fixture runner counter just_victim ambient_shims first second
  local first_identity second_identity
  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/nested-just-counter"
  just_victim="$fixture/target/hostile-just-tempdir"
  ambient_shims=/usr/local/share/mise/shims
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
    PATH="$ambient_shims:$PATH" \
    "$runner" -- just outer)"
  second="$(cd "$fixture" && JUST_TEMPDIR="$fixture/target/second-hostile-just-tempdir" \
    PATH="$ambient_shims:$PATH" \
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
}
