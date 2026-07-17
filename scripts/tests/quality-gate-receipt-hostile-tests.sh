#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
source "$repo_root/scripts/tests/quality-gate-receipt-tests.sh"

run_hostile_state() {
  local fixture counter runner victim output identity lock receipt code started elapsed

  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  victim="${test_root}/state-victim"
  mkdir "$victim"
  printf 'sentinel\n' >"$victim/sentinel"
  ln -s "$victim" "${fixture}/.csa/state/quality-gate-receipts"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq hostile-state-directory-symlink-status executed \
    "$(printf '%s' "$output" | json_field status)"
  assert_eq hostile-state-directory-symlink-reason state_untrusted \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq hostile-state-directory-symlink-victim-count 1 \
    "$(find "$victim" -mindepth 1 -maxdepth 1 | wc -l)"
  assert_eq hostile-state-directory-symlink-sentinel sentinel "$(<"$victim/sentinel")"
  echo "PASS hostile-state-directory-symlink"

  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  mkdir -p "${fixture}/.csa/state/quality-gate-receipts"
  victim="${test_root}/collection-lock-victim"
  printf 'do-not-truncate\n' >"$victim"
  ln -s "$victim" "${fixture}/.csa/state/quality-gate-receipts/collection.lock"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq hostile-collection-lock-symlink-victim do-not-truncate "$(<"$victim")"
  assert_eq hostile-collection-lock-symlink-reason state_untrusted \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  echo "PASS hostile-collection-lock-symlink"

  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  identity="$(printf '%s' "$output" | json_field receipt_identity)"
  rm "${fixture}/.csa/state/quality-gate-receipts/${identity}.json" \
    "${fixture}/.csa/state/quality-gate-receipts/${identity}.lock"
  victim="${test_root}/identity-lock-victim"
  printf 'do-not-truncate\n' >"$victim"
  ln -s "$victim" "${fixture}/.csa/state/quality-gate-receipts/${identity}.lock"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq hostile-identity-lock-symlink-victim do-not-truncate "$(<"$victim")"
  assert_eq hostile-identity-lock-symlink-reason state_untrusted \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  echo "PASS hostile-identity-lock-symlink"

  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  identity="$(printf '%s' "$output" | json_field receipt_identity)"
  chmod 0777 "${fixture}/.csa/state/quality-gate-receipts"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq hostile-state-mode-reason state_untrusted \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq hostile-state-mode-gate-runs 2 "$(wc -c <"$counter")"
  chmod 0700 "${fixture}/.csa/state/quality-gate-receipts"
  rm "${fixture}/.csa/state/quality-gate-receipts/${identity}.json"
  chmod 0666 "${fixture}/.csa/state/quality-gate-receipts/${identity}.lock"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq hostile-lock-mode-reason state_untrusted \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq hostile-lock-mode-gate-runs 3 "$(wc -c <"$counter")"
  echo "PASS hostile-state-and-lock-mode"

  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  mkdir -p "${fixture}/.csa/state/quality-gate-receipts"
  : >"${fixture}/.csa/state/quality-gate-receipts/collection.lock"
  python3 - "${fixture}/.csa/state/quality-gate-receipts/collection.lock" \
    "${fixture}/.csa/state/collection-lock-ready" <<'PY' &
import fcntl, pathlib, sys, time
with open(sys.argv[1], "r+b", buffering=0) as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    pathlib.Path(sys.argv[2]).write_text("ready\n", encoding="utf-8")
    time.sleep(12)
PY
  lock=$!
  register_child "$lock"
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    [ -e "${fixture}/.csa/state/collection-lock-ready" ] && break
    sleep 0.1
  done
  assert_path_exists hostile-collection-lock-timeout-ready \
    "${fixture}/.csa/state/collection-lock-ready"
  started="$(date +%s)"
  set +e
  output="$(cd "$fixture" && timeout 7 "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  code=$?
  set -e
  elapsed="$(( $(date +%s) - started ))"
  kill "$lock" 2>/dev/null || true
  wait "$lock" 2>/dev/null || true
  unregister_child "$lock"
  assert_eq hostile-collection-lock-timeout-exit 0 "$code"
  assert_num_lt hostile-collection-lock-timeout-elapsed 7 "$elapsed"
  assert_eq hostile-collection-lock-timeout-reason lock_timeout \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  echo "PASS hostile-collection-lock-timeout"

  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  identity="$(printf '%s' "$output" | json_field receipt_identity)"
  rm "${fixture}/.csa/state/quality-gate-receipts/${identity}.json"
  python3 - "${fixture}/.csa/state/quality-gate-receipts/${identity}.lock" \
    "${fixture}/.csa/state/lock-ready" <<'PY' &
import fcntl, pathlib, sys, time
with open(sys.argv[1], "r+b", buffering=0) as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    pathlib.Path(sys.argv[2]).write_text("ready\n", encoding="utf-8")
    time.sleep(12)
PY
  lock=$!
  register_child "$lock"
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    [ -e "${fixture}/.csa/state/lock-ready" ] && break
    sleep 0.1
  done
  assert_path_exists hostile-identity-lock-timeout-ready \
    "${fixture}/.csa/state/lock-ready"
  started="$(date +%s)"
  set +e
  output="$(cd "$fixture" && timeout 7 "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  code=$?
  set -e
  elapsed="$(( $(date +%s) - started ))"
  kill "$lock" 2>/dev/null || true
  wait "$lock" 2>/dev/null || true
  unregister_child "$lock"
  assert_eq hostile-identity-lock-timeout-exit 0 "$code"
  assert_num_lt hostile-identity-lock-timeout-elapsed 7 "$elapsed"
  assert_eq hostile-identity-lock-timeout-reason lock_timeout \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  echo "PASS hostile-lock-timeout"

  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  identity="$(printf '%s' "$output" | json_field receipt_identity)"
  receipt="${fixture}/.csa/state/quality-gate-receipts/${identity}.json"
  chmod 0666 "$receipt"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq hostile-receipt-mode-reason receipt_mode_unsafe \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  chmod 0600 "$receipt"
  printf '%*s' 70000 '' >"$receipt"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq hostile-receipt-size-reason receipt_too_large \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  rm -f "$receipt"
  victim="${test_root}/hard-link-victim"
  printf '{"secret":"must-not-be-consumed"}\n' >"$victim"
  ln "$victim" "$receipt"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq hostile-receipt-hard-link-reason receipt_hard_link \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq hostile-receipt-hard-link-victim \
    '{"secret":"must-not-be-consumed"}' "$(<"$victim")"
  rm "$receipt"
  victim="${test_root}/receipt-symlink-victim"
  printf 'arbitrary-target-bytes\n' >"$victim"
  ln -s "$victim" "$receipt"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq hostile-receipt-symlink-reason receipt_symlink \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq hostile-receipt-symlink-victim arbitrary-target-bytes "$(<"$victim")"
  echo "PASS hostile-receipt-type-size-mode"
}


run_hostile_state
