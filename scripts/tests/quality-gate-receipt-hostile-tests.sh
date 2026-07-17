#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
source "$repo_root/scripts/tests/quality-gate-receipt-tests.sh"

run_hostile_state() {
  local fixture counter runner victim output identity lock receipt code started elapsed

  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  victim="${test_root}/state-victim"
  mkdir "$victim"
  printf 'sentinel\n' >"$victim/sentinel"
  ln -s "$victim" "${fixture}/.csa/state/quality-gate-receipts"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  test "$(printf '%s' "$output" | json_field status)" = executed
  test "$(printf '%s' "$output" | json_field rejection_reason)" = state_untrusted
  test "$(find "$victim" -mindepth 1 -maxdepth 1 | wc -l)" -eq 1
  test "$(cat "$victim/sentinel")" = sentinel
  echo "PASS hostile-state-directory-symlink"

  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  mkdir -p "${fixture}/.csa/state/quality-gate-receipts"
  victim="${test_root}/collection-lock-victim"
  printf 'do-not-truncate\n' >"$victim"
  ln -s "$victim" "${fixture}/.csa/state/quality-gate-receipts/collection.lock"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  test "$(cat "$victim")" = do-not-truncate
  test "$(printf '%s' "$output" | json_field rejection_reason)" = state_untrusted
  echo "PASS hostile-collection-lock-symlink"

  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  identity="$(printf '%s' "$output" | json_field receipt_identity)"
  rm "${fixture}/.csa/state/quality-gate-receipts/${identity}.json" \
    "${fixture}/.csa/state/quality-gate-receipts/${identity}.lock"
  victim="${test_root}/identity-lock-victim"
  printf 'do-not-truncate\n' >"$victim"
  ln -s "$victim" "${fixture}/.csa/state/quality-gate-receipts/${identity}.lock"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  test "$(cat "$victim")" = do-not-truncate
  test "$(printf '%s' "$output" | json_field rejection_reason)" = state_untrusted
  echo "PASS hostile-identity-lock-symlink"

  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  identity="$(printf '%s' "$output" | json_field receipt_identity)"
  chmod 0777 "${fixture}/.csa/state/quality-gate-receipts"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  test "$(printf '%s' "$output" | json_field rejection_reason)" = state_untrusted
  test "$(wc -c <"$counter")" -eq 2
  chmod 0700 "${fixture}/.csa/state/quality-gate-receipts"
  rm "${fixture}/.csa/state/quality-gate-receipts/${identity}.json"
  chmod 0666 "${fixture}/.csa/state/quality-gate-receipts/${identity}.lock"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  test "$(printf '%s' "$output" | json_field rejection_reason)" = state_untrusted
  test "$(wc -c <"$counter")" -eq 3
  echo "PASS hostile-state-and-lock-mode"

  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
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
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    [ -e "${fixture}/.csa/state/collection-lock-ready" ] && break
    sleep 0.1
  done
  test -e "${fixture}/.csa/state/collection-lock-ready"
  started="$(date +%s)"
  set +e
  output="$(cd "$fixture" && timeout 7 "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  code=$?
  set -e
  elapsed="$(( $(date +%s) - started ))"
  kill "$lock" 2>/dev/null || true
  wait "$lock" 2>/dev/null || true
  test "$code" -eq 0
  test "$elapsed" -lt 7
  test "$(printf '%s' "$output" | json_field rejection_reason)" = lock_timeout
  echo "PASS hostile-collection-lock-timeout"

  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
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
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    [ -e "${fixture}/.csa/state/lock-ready" ] && break
    sleep 0.1
  done
  test -e "${fixture}/.csa/state/lock-ready"
  started="$(date +%s)"
  set +e
  output="$(cd "$fixture" && timeout 7 "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  code=$?
  set -e
  elapsed="$(( $(date +%s) - started ))"
  kill "$lock" 2>/dev/null || true
  wait "$lock" 2>/dev/null || true
  test "$code" -eq 0
  test "$elapsed" -lt 7
  test "$(printf '%s' "$output" | json_field rejection_reason)" = lock_timeout
  echo "PASS hostile-lock-timeout"

  fixture="$(new_fixture)"
  counter="${fixture}/.csa/state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  identity="$(printf '%s' "$output" | json_field receipt_identity)"
  receipt="${fixture}/.csa/state/quality-gate-receipts/${identity}.json"
  chmod 0666 "$receipt"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  test "$(printf '%s' "$output" | json_field rejection_reason)" = receipt_mode_unsafe
  chmod 0600 "$receipt"
  printf '%*s' 70000 '' >"$receipt"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  test "$(printf '%s' "$output" | json_field rejection_reason)" = receipt_too_large
  rm -f "$receipt"
  victim="${test_root}/hard-link-victim"
  printf '{"secret":"must-not-be-consumed"}\n' >"$victim"
  ln "$victim" "$receipt"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  test "$(printf '%s' "$output" | json_field rejection_reason)" = receipt_hard_link
  test "$(cat "$victim")" = '{"secret":"must-not-be-consumed"}'
  rm "$receipt"
  victim="${test_root}/receipt-symlink-victim"
  printf 'arbitrary-target-bytes\n' >"$victim"
  ln -s "$victim" "$receipt"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  test "$(printf '%s' "$output" | json_field rejection_reason)" = receipt_symlink
  test "$(cat "$victim")" = arbitrary-target-bytes
  echo "PASS hostile-receipt-type-size-mode"
}


run_hostile_state
