#!/usr/bin/env bash
set -euo pipefail

run_contract_suite() {
  local suite="$1" expected="$2" code output count duplicates
  if output="$(bash "$suite" 2>&1)"; then
    code=0
  else
    code=$?
  fi
  if [ "$code" -ne 0 ]; then
    printf '%s\n' "$output" >&2
    printf 'FAIL contract-suite-%s expected=exit-0 actual=exit-%s\n' \
      "${suite##*/}" "$code" >&2
    return "$code"
  fi
  count="$(grep -c '^PASS ' <<<"$output" || true)"
  duplicates="$(awk '/^PASS / { print $2 }' <<<"$output" | sort | uniq -d)"
  if [ "$count" -ne "$expected" ] || [ -n "$duplicates" ]; then
    printf '%s\n' "$output" >&2
    printf 'FAIL contract-suite-%s expected=unique-pass-%s actual=pass-%s\n' \
      "${suite##*/}" "$expected" "$count" >&2
    return 1
  fi
  printf '%s\n' "$output"
}

# Exact ratchet: 45 core + 7 hostile + 7 isolation + 1 pre-push + 2
# dev2merge runtime contracts = 62 independently named PASS cases.
run_contract_suite scripts/tests/quality-gate-receipt-tests.sh 45
run_contract_suite scripts/tests/quality-gate-receipt-hostile-tests.sh 7
run_contract_suite scripts/tests/quality-gate-isolation-tests.sh 7
run_contract_suite scripts/tests/pre-push-quality-gates-tests.sh 1
run_contract_suite scripts/tests/dev2merge-quality-gate-receipt-tests.sh 2
