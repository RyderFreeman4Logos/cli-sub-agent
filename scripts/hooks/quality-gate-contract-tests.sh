#!/usr/bin/env bash
set -euo pipefail

run_contract_suite() {
  local suite="$1" expected="$2" code count duplicates capture filtered bounded
  local diagnostic_pattern diagnostic_status diagnostic_marker
  local diagnostic_bytes status_bytes marker_bytes diagnostic_budget
  diagnostic_pattern='^(ERROR quality-gate status=[a-z0-9_-]+ exit=[0-9]+ reason=[a-z0-9_-]+|FAIL contract-case suite=[a-zA-Z0-9_.-]+ case=[a-zA-Z0-9_.-]+ exit=[0-9]+|FAIL [a-zA-Z0-9][a-zA-Z0-9_.-]* expected=[a-zA-Z0-9][a-zA-Z0-9_.-]* actual=[a-zA-Z0-9][a-zA-Z0-9_.-]*)$'
  capture="$(mktemp "${TMPDIR:-/tmp}/quality-gate-contract.XXXXXX")"
  if bash "$suite" >"$capture" 2>&1; then
    code=0
  else
    code=$?
  fi
  if [ "$code" -ne 0 ]; then
    filtered="$(mktemp "${TMPDIR:-/tmp}/quality-gate-contract.filtered.XXXXXX")"
    bounded="$(mktemp "${TMPDIR:-/tmp}/quality-gate-contract.bounded.XXXXXX")"
    LC_ALL=C grep -aE -- "$diagnostic_pattern" "$capture" \
      | tail -20 >"$filtered" || true
    if [ ! -s "$filtered" ]; then
      printf 'FAIL contract-case suite=%s case=unreported exit=%s\n' \
        "${suite##*/}" "$code" >"$filtered"
    fi
    diagnostic_status="FAIL contract-suite-${suite##*/} expected=exit-0 actual=exit-${code}"
    diagnostic_marker='...[diagnostic truncated]...'
    diagnostic_bytes="$(LC_ALL=C wc -c <"$filtered" | tr -d '[:space:]')"
    status_bytes="$(printf '%s\n' "$diagnostic_status" | LC_ALL=C wc -c | tr -d '[:space:]')"
    diagnostic_budget=$((16384 - status_bytes))
    if [ "$diagnostic_bytes" -le "$diagnostic_budget" ]; then
      cat "$filtered" >"$bounded"
    else
      marker_bytes="$(printf '\n%s\n' "$diagnostic_marker" | LC_ALL=C wc -c | tr -d '[:space:]')"
      head -c "$((diagnostic_budget - marker_bytes))" "$filtered" >"$bounded"
      printf '\n%s\n' "$diagnostic_marker" >>"$bounded"
    fi
    cat "$bounded" >&2
    printf '%s\n' "$diagnostic_status" >&2
    rm -f "$filtered" "$bounded" "$capture"
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
  # Exact ratchet: 46 core + 1 intentional-local + 8 hostile + 19 isolation + 8
  # pre-push + 3 dev2merge runtime contracts = 85 independently named PASS cases.
  run_contract_suite scripts/tests/quality-gate-receipt-tests.sh 46
  run_contract_suite scripts/tests/quality-gate-receipt-intentional-local-tests.sh 1
  run_contract_suite scripts/tests/quality-gate-receipt-hostile-tests.sh 8
  run_contract_suite scripts/tests/quality-gate-isolation-tests.sh 19
  run_contract_suite scripts/tests/pre-push-quality-gates-tests.sh 8
  run_contract_suite scripts/tests/dev2merge-quality-gate-receipt-tests.sh 3
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
