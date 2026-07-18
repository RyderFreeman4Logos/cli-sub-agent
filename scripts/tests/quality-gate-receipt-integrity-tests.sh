#!/usr/bin/env bash
# Sourced by quality-gate-receipt-tests.sh after fixture helpers are defined.

current_receipt() {
  find "$1/.csa/state/quality-gate-receipts" -maxdepth 1 -type f -name '*.json' | head -1
}

assert_corruption_reexecutes() {
  local name="$1" mutation="$2" fixture counter runner receipt output
  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  (cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter") >/dev/null
  receipt="$(current_receipt "$fixture")"
  # Trusted mutation snippets reference this target through eval.
  export receipt
  eval "$mutation"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq "integrity-${name}-status" executed \
    "$(printf '%s' "$output" | json_field status)"
  assert_nonempty "integrity-${name}-reason" \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq "integrity-${name}-gate-runs" 2 "$(wc -c <"$counter")"
  echo "PASS integrity-$name"
}

assert_single_json() {
  local record="$1" status
  assert_eq structured-result-line-count 1 "$(printf '%s\n' "$record" | wc -l)"
  status="$(printf '%s' "$record" | json_field status)"
  case "$status" in
    executed | reused | gate_failed) ;;
    *) _receipt_test_fail structured-result-status allowed-status "$status" ;;
  esac
  assert_not_matches structured-result-redaction \
    '/tmp/|example\.invalid|credential|secret-token' "$record"
}

wait_for_pid_bounded() {
  local pid="$1"
  if ! timeout 10 tail --pid="$pid" -f /dev/null; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    unregister_child "$pid"
    _receipt_test_fail fixture-process-timeout completed-within-10s timed-out
    return 1
  fi
  local code
  if wait "$pid"; then
    unregister_child "$pid"
    return 0
  else
    code=$?
  fi
  unregister_child "$pid"
  _receipt_test_fail fixture-process-exit 0 "$code"
}

run_integrity_concurrency() {
  assert_corruption_reexecutes malformed 'printf "{truncated" >"$receipt"'
  assert_corruption_reexecutes unknown-schema 'python3 - "$receipt" <<'"'"'PY'"'"'
import json,sys
p=sys.argv[1]; value=json.load(open(p)); value["schema_version"]=999; open(p,"w").write(json.dumps(value))
PY'
  assert_corruption_reexecutes missing-field 'python3 - "$receipt" <<'"'"'PY'"'"'
import json,sys
p=sys.argv[1]; value=json.load(open(p)); del value["status"]; open(p,"w").write(json.dumps(value))
PY'
  assert_corruption_reexecutes non-pass 'python3 - "$receipt" <<'"'"'PY'"'"'
import json,sys
p=sys.argv[1]; value=json.load(open(p)); value["status"]="FAIL"; open(p,"w").write(json.dumps(value))
PY'
  assert_corruption_reexecutes content-digest 'python3 - "$receipt" <<'"'"'PY'"'"'
import json,sys
p=sys.argv[1]; value=json.load(open(p)); value["receipt_digest"]="0"*64; open(p,"w").write(json.dumps(value))
PY'
  assert_corruption_reexecutes filename-digest 'python3 - "$receipt" <<'"'"'PY'"'"'
import json,sys
p=sys.argv[1]; value=json.load(open(p)); value["identity"]="f"*64; open(p,"w").write(json.dumps(value))
PY'
  assert_corruption_reexecutes symlink 'target="${receipt}.target"; mv "$receipt" "$target"; ln -s "$target" "$receipt"'
  assert_corruption_reexecutes non-file 'rm -f "$receipt"; mkdir "$receipt"'

  local fixture counter runner output code receipt_dir diagnostic_file diagnostic
  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  printf '#!/usr/bin/env bash\nexit 7\n' >"$fixture/scripts/hooks/failing-gate.sh"
  chmod +x "$fixture/scripts/hooks/failing-gate.sh"
  git -C "$fixture" add scripts/hooks/failing-gate.sh
  git -C "$fixture" commit -qm "test: add failing gate"
  diagnostic_file="$fixture/target/failing-gate.stderr"
  set +e
  output="$(cd "$fixture" && \
    "$runner" -- scripts/hooks/failing-gate.sh 2>"$diagnostic_file")"
  code=$?
  set -e
  diagnostic="$(<"$diagnostic_file")"
  assert_eq integrity-gate-failure-exit 7 "$code"
  assert_eq integrity-gate-failure-status gate_failed \
    "$(printf '%s' "$output" | json_field status)"
  assert_single_json "$output"
  assert_eq integrity-gate-failure-diagnostic \
    'ERROR quality-gate status=gate_failed exit=7 reason=gate_exit_nonzero' \
    "$diagnostic"
  assert_empty integrity-gate-failure-receipt "$(current_receipt "$fixture")"
  echo "PASS integrity-gate-failure"

  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  printf '#!/usr/bin/env bash\nkill -TERM "$PPID"\nexit 143\n' >"$fixture/scripts/hooks/signal-gate.sh"
  chmod +x "$fixture/scripts/hooks/signal-gate.sh"
  git -C "$fixture" add scripts/hooks/signal-gate.sh
  git -C "$fixture" commit -qm "test: add signal gate"
  diagnostic_file="$fixture/target/signal-gate.stderr"
  set +e
  output="$(cd "$fixture" && \
    "$runner" -- scripts/hooks/signal-gate.sh 2>"$diagnostic_file")"
  code=$?
  set -e
  diagnostic="$(<"$diagnostic_file")"
  assert_ne integrity-signal-exit 0 "$code"
  assert_eq integrity-signal-diagnostic \
    'ERROR quality-gate status=gate_failed exit=143 reason=gate_exit_nonzero' \
    "$diagnostic"
  assert_empty integrity-signal-receipt "$(current_receipt "$fixture")"
  echo "PASS integrity-signal"

  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  set +e
  (cd "$fixture" && CSA_QUALITY_GATE_TEST_FAULT=crash-before-publish \
    "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter") >/dev/null 2>&1
  code=$?
  set -e
  assert_ne integrity-crash-before-rename-exit 0 "$code"
  assert_empty integrity-crash-before-rename-receipt "$(current_receipt "$fixture")"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq integrity-crash-before-rename-recovery-status executed \
    "$(printf '%s' "$output" | json_field status)"
  echo "PASS integrity-crash-before-rename"

  fixture="$(new_fixture)"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  receipt_dir="${fixture}/.csa/state/quality-gate-receipts"
  printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>"$1"\nprintf ready >"$2"\nwhile [ ! -e "$3" ]; do sleep 0.02; done\n' >"$fixture/scripts/hooks/blocking-gate.sh"
  chmod +x "$fixture/scripts/hooks/blocking-gate.sh"
  git -C "$fixture" add scripts/hooks/blocking-gate.sh
  git -C "$fixture" commit -qm "test: add blocking gate"
  (
    cd "$fixture"
    exec "$runner" -- scripts/hooks/blocking-gate.sh "$counter" \
      target/quality-gate-test-state/ready \
      target/quality-gate-test-state/release >.csa/state/writer-one.json
  ) &
  local writer_one=$!
  register_child "$writer_one"
  if ! timeout 5 bash -c 'until [ -e "$1" ]; do sleep 0.02; done' _ \
    "$fixture/target/quality-gate-test-state/ready"; then
    kill -KILL "$writer_one" 2>/dev/null || true
    wait "$writer_one" 2>/dev/null || true
    echo "timed out waiting for the first fixture writer" >&2
    return 1
  fi
  (
    cd "$fixture"
    exec "$runner" -- scripts/hooks/blocking-gate.sh "$counter" \
      target/quality-gate-test-state/ready \
      target/quality-gate-test-state/release >.csa/state/writer-two.json
  ) &
  local writer_two=$!
  register_child "$writer_two"
  touch "$fixture/target/quality-gate-test-state/release"
  wait_for_pid_bounded "$writer_one"
  wait_for_pid_bounded "$writer_two"
  assert_eq integrity-concurrency-initial-gate-runs 1 "$(wc -c <"$counter")"
  assert_eq integrity-concurrency-receipt-count 1 \
    "$(find "$receipt_dir" -maxdepth 1 -type f -name '*.json' | wc -l)"
  assert_single_json "$(cat "$fixture/.csa/state/writer-one.json")"
  assert_single_json "$(cat "$fixture/.csa/state/writer-two.json")"
  local writers=()
  for _ in 1 2 3 4 5 6; do
    (
      cd "$fixture"
      exec "$runner" -- scripts/hooks/blocking-gate.sh "$counter" \
        target/quality-gate-test-state/ready \
        target/quality-gate-test-state/release >/dev/null
    ) &
    writers+=("$!")
    register_child "$!"
  done
  local writer
  for writer in "${writers[@]}"; do
    wait_for_pid_bounded "$writer"
  done
  assert_eq integrity-concurrency-final-gate-runs 1 "$(wc -c <"$counter")"
  echo "PASS integrity-concurrency"
}
