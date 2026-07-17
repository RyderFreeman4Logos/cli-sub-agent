#!/usr/bin/env bash
set -euo pipefail

run_contract_suite() {
  local suite="$1" code
  if bash "$suite"; then
    return 0
  else
    code=$?
  fi
  printf 'FAIL contract-suite-%s expected=exit-0 actual=exit-%s\n' \
    "${suite##*/}" "$code" >&2
  return "$code"
}

# Exercise receipt contracts in fake-gate fixtures without recursive host gates.
run_contract_suite scripts/tests/quality-gate-receipt-tests.sh
run_contract_suite scripts/tests/quality-gate-receipt-hostile-tests.sh
run_contract_suite scripts/tests/pre-push-quality-gates-tests.sh
run_contract_suite scripts/tests/dev2merge-quality-gate-receipt-tests.sh
