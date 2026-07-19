#!/usr/bin/env bash
set -euo pipefail

run_contract_suite() {
  local suite="$1" expected="$2" code count duplicates capture diagnostic
  local diagnostic_pattern
  diagnostic_pattern='^(ERROR quality-gate status=[a-z0-9_-]+ exit=[0-9]+ reason=[a-z0-9_-]+|FAIL contract-case suite=[a-zA-Z0-9_.-]+ case=[a-zA-Z0-9_.-]+ exit=[0-9]+|FAIL [a-zA-Z0-9][a-zA-Z0-9_.-]* expected=[a-zA-Z0-9][a-zA-Z0-9_.-]* actual=[a-zA-Z0-9][a-zA-Z0-9_.-]*)$'
  capture="$(mktemp "${TMPDIR:-/tmp}/quality-gate-contract.XXXXXX")"
  if bash "$suite" >"$capture" 2>&1; then
    code=0
  else
    code=$?
  fi
  if [ "$code" -ne 0 ]; then
    diagnostic="$(
      tail -c 16384 "$capture" \
        | grep -E "$diagnostic_pattern" \
        | tail -20 || true
    )"
    if [ -n "$diagnostic" ]; then
      printf '%s\n' "$diagnostic" >&2
    else
      printf 'FAIL contract-case suite=%s case=unreported exit=%s\n' \
        "${suite##*/}" "$code" >&2
    fi
    printf 'FAIL contract-suite-%s expected=exit-0 actual=exit-%s\n' \
      "${suite##*/}" "$code" >&2
    rm -f "$capture"
    return "$code"
  fi
  count="$(grep -c '^PASS ' "$capture" || true)"
  duplicates="$(awk '/^PASS / { print $2 }' "$capture" | sort | uniq -d)"
  if [ "$count" -ne "$expected" ] || [ -n "$duplicates" ]; then
    tail -c 16384 "$capture" >&2
    printf 'FAIL contract-suite-%s expected=unique-pass-%s actual=pass-%s\n' \
      "${suite##*/}" "$expected" "$count" >&2
    rm -f "$capture"
    return 1
  fi
  cat "$capture"
  rm -f "$capture"
}

run_quality_gate_contract_suites() {
  # Exact ratchet: 45 core + 7 hostile + 15 isolation + 8 pre-push + 2
  # dev2merge runtime contracts = 77 independently named PASS cases.
  run_contract_suite scripts/tests/quality-gate-receipt-tests.sh 45
  run_contract_suite scripts/tests/quality-gate-receipt-hostile-tests.sh 7
  run_contract_suite scripts/tests/quality-gate-isolation-tests.sh 15
  run_contract_suite scripts/tests/pre-push-quality-gates-tests.sh 8
  run_contract_suite scripts/tests/dev2merge-quality-gate-receipt-tests.sh 2
}

quality_gate_contract_tests_main() {
  if [ "$#" -ne 0 ]; then
    printf '%s\n' \
      'ERROR quality-gate-contract-tests accepts no arguments' \
      'usage: bash scripts/hooks/quality-gate-contract-tests.sh' >&2
    return 2
  fi
  run_quality_gate_contract_suites
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  quality_gate_contract_tests_main "$@"
fi
