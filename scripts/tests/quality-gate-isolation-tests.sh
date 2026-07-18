#!/usr/bin/env bash
set -euo pipefail
shopt -s inherit_errexit

export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
export GIT_CONFIG_NOSYSTEM=1

repo_root="$(git rev-parse --show-toplevel)"
source "$repo_root/scripts/tests/quality-gate-test-assertions.sh"
receipt_contract_install_failure_trap quality-gate-isolation-tests.sh
scenario="${1:-all}"
mkdir -p "$repo_root/drafts"
test_root="$(realpath -e "$(mktemp -d "$repo_root/drafts/quality-gate-isolation.XXXXXX")")"
owned_pids=()

cleanup() {
  local pid
  for pid in "${owned_pids[@]}"; do
    kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  rm -rf -- "$test_root"
}
trap cleanup EXIT

new_isolation_fixture() {
  local fixture
  fixture="$(mktemp -d "$test_root/fixture.XXXXXX")"
  git -C "$fixture" init -q
  git -C "$fixture" config user.name "Quality Gate Isolation Tests"
  git -C "$fixture" config user.email "quality-gate-isolation@example.invalid"
  git -C "$fixture" remote add origin https://example.invalid/isolation.git
  mkdir -p "$fixture/scripts/hooks" "$fixture/.csa/state" "$fixture/target"
  printf '/.csa/state/\n/target/\n' >"$fixture/.gitignore"
  cp "$repo_root/scripts/cargo-env-normalize.sh" "$fixture/scripts/"
  cp "$repo_root/scripts/quality-gate-state.py" "$fixture/scripts/"
  cp "$repo_root/scripts/quality_gate_provenance.py" "$fixture/scripts/"
  cp "$repo_root/scripts/quality_gate_secure_state.py" "$fixture/scripts/"
  if [ -f "$repo_root/scripts/quality_gate_sandbox.py" ]; then
    cp "$repo_root/scripts/quality_gate_sandbox.py" "$fixture/scripts/"
  fi
  if [ -f "$repo_root/scripts/quality_gate_process.py" ]; then
    cp "$repo_root/scripts/quality_gate_process.py" "$fixture/scripts/"
  fi
  if [ -f "$repo_root/scripts/quality_gate_environment.py" ]; then
    cp "$repo_root/scripts/quality_gate_environment.py" "$fixture/scripts/"
  fi
  if [ -f "$repo_root/scripts/quality_gate_toolchain.py" ]; then
    cp "$repo_root/scripts/quality_gate_toolchain.py" "$fixture/scripts/"
  fi
  cp "$repo_root/scripts/hooks/quality-gate-receipt.sh" "$fixture/scripts/hooks/"
  cp "$repo_root/rust-toolchain.toml" "$fixture/"
  printf '[workspace]\n' >"$fixture/Cargo.toml"
  printf '# lock\n' >"$fixture/Cargo.lock"
  printf '# weave\n' >"$fixture/weave.lock"
  printf 'quality-gates:\n    true\n' >"$fixture/justfile"
  printf 'pre-push: {}\n' >"$fixture/lefthook.yml"
  printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>"$1"\n' \
    >"$fixture/scripts/hooks/true-gate.sh"
  chmod +x "$fixture/scripts/hooks/true-gate.sh"
  git -C "$fixture" add .gitignore Cargo.toml Cargo.lock weave.lock justfile \
    lefthook.yml rust-toolchain.toml scripts
  git -C "$fixture" commit -qm "test: initialize isolation fixture"
  printf '%s\n' "$fixture"
}

json_field() {
  python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$1"
}

current_receipt_count() {
  find "$1/.csa/state/quality-gate-receipts" -maxdepth 1 -type f \
    -name '*.json' 2>/dev/null | wc -l
}

source "$repo_root/scripts/tests/quality-gate-offline-toolchain-tests.sh"

run_digest_correct_forgery() {
  local fixture runner counter first second first_code second_code second_status
  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  cat >"$fixture/scripts/hooks/forging-gate.py" <<'PY'
#!/usr/bin/env python3
import json
import os
import subprocess
import sys
from pathlib import Path

repo = Path.cwd()
sys.path.insert(0, str(repo / "scripts"))
from quality_gate_secure_state import IMPLEMENTATION_VERSION, SCHEMA_VERSION, sha256_bytes

counter = Path(sys.argv[1])
counter.write_text(counter.read_text() + "x" if counter.exists() else "x")
command = ("scripts/hooks/forging-gate.py", sys.argv[1])
manifest = subprocess.check_output(
    (
        "scripts/cargo-env-normalize.sh",
        sys.executable,
        "scripts/quality-gate-state.py",
        "collect",
        "--repo",
        str(repo),
        "--",
        *command,
    )
)
identity = sha256_bytes(manifest)
payload = {
    "identity": identity,
    "implementation_version": IMPLEMENTATION_VERSION,
    "manifest": manifest.decode(),
    "manifest_sha256": identity,
    "schema_version": SCHEMA_VERSION,
    "status": "PASS",
}
payload["receipt_digest"] = sha256_bytes(
    json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
)
state = repo / ".csa/state/quality-gate-receipts"
state.mkdir(parents=True, exist_ok=True)
receipt = state / f"{identity}.json"
receipt.write_text(json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n")
receipt.chmod(0o600)
raise SystemExit(7)
PY
  chmod +x "$fixture/scripts/hooks/forging-gate.py"
  git -C "$fixture" add scripts/hooks/forging-gate.py
  git -C "$fixture" commit -qm "test: add digest-correct forger"

  set +e
  first="$(cd "$fixture" && "$runner" -- scripts/hooks/forging-gate.py target/gate-counter)"
  first_code=$?
  second="$(cd "$fixture" && "$runner" -- scripts/hooks/forging-gate.py target/gate-counter)"
  second_code=$?
  set -e
  second_status="$(printf '%s' "$second" | json_field status)"
  assert_eq isolation-forgery-first-exit 7 "$first_code"
  assert_eq isolation-forgery-second-exit 7 "$second_code"
  assert_ne isolation-forgery-second-status reused "$second_status"
  assert_eq isolation-forgery-gate-runs 2 "$(wc -c <"$counter")"
  assert_eq isolation-forgery-receipt-count 0 "$(current_receipt_count "$fixture")"
  echo "PASS isolation-digest-correct-forgery"
}

run_state_capability_isolation() {
  local fixture runner counter output
  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  printf 'protected\n' >"$fixture/.csa/state/host-protected"
  cat >"$fixture/scripts/hooks/state-attacker.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf x >>"$1"
printf poisoned >.csa/state/relative-write 2>/dev/null || true
printf poisoned >"$PWD/.csa/state/absolute-write" 2>/dev/null || true
mv .csa/state/host-protected .csa/state/host-protected-moved 2>/dev/null || true
for descriptor in /proc/[0-9]*/fd/*; do
  target="$(readlink "$descriptor" 2>/dev/null || true)"
  case "$target" in
    *quality-gate-receipts*) printf exposed >"$2"; break ;;
  esac
done
mkdir -p "${TMPDIR:-/tmp}/quality-gate-private-write"
SH
  chmod +x "$fixture/scripts/hooks/state-attacker.sh"
  git -C "$fixture" add scripts/hooks/state-attacker.sh
  git -C "$fixture" commit -qm "test: add state capability attacker"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/state-attacker.sh \
    target/gate-counter target/proc-fd-exposed)"
  assert_eq isolation-state-status executed \
    "$(printf '%s' "$output" | json_field status)"
  assert_eq isolation-state-counter 1 "$(wc -c <"$counter")"
  assert_path_absent isolation-state-relative-write \
    "$fixture/.csa/state/relative-write"
  assert_path_absent isolation-state-absolute-write \
    "$fixture/.csa/state/absolute-write"
  assert_path_exists isolation-state-rename-source \
    "$fixture/.csa/state/host-protected"
  assert_path_absent isolation-state-rename-target \
    "$fixture/.csa/state/host-protected-moved"
  assert_path_absent isolation-state-proc-fd "$fixture/target/proc-fd-exposed"
  echo "PASS isolation-state-capabilities"
}

process_start_time() {
  awk '{print $22}' "/proc/$1/stat" 2>/dev/null || true
}

wait_for_process_identity_gone() {
  local pid="$1" start_time="$2" deadline=$((SECONDS + 5)) current
  while [ "$SECONDS" -lt "$deadline" ]; do
    current="$(process_start_time "$pid")"
    if [ -z "$current" ] || [ "$current" != "$start_time" ]; then
      return 0
    fi
    sleep 0.05
  done
  return 1
}

find_process_by_token() {
  local token="$1" deadline=$((SECONDS + 10)) pid argument path
  local -a arguments
  while [ "$SECONDS" -lt "$deadline" ]; do
    for path in /proc/[0-9]*/cmdline; do
      pid="${path#/proc/}"
      pid="${pid%/cmdline}"
      arguments=()
      mapfile -d '' -t arguments <"$path" 2>/dev/null || true
      [ "${arguments[1]:-}" = "-c" ] || continue
      for argument in "${arguments[@]}"; do
        if [ "$argument" = "$token" ]; then
          printf '%s\n' "$pid"
          return 0
        fi
      done
    done
    sleep 0.05
  done
  return 1
}

wait_for_file() {
  local path="$1" deadline=$((SECONDS + 10))
  while [ ! -s "$path" ]; do
    if [ "$SECONDS" -ge "$deadline" ]; then
      _receipt_test_fail isolation-process-ready ready timed-out
      return 1
    fi
    sleep 0.05
  done
}

run_process_tree_termination() {
  local fixture runner token runner_pid descendant descendant_start code
  local requested_signal label
  for requested_signal in TERM HUP INT; do
    label="${requested_signal,,}"
    fixture="$(new_isolation_fixture)"
    runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
    cat >"$fixture/scripts/hooks/stubborn-tree.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
trap '' HUP INT TERM
setsid bash -c 'trap "" HUP INT TERM; while :; do sleep 1; done' "$1" &
printf ready >target/gate-ready
wait
SH
    chmod +x "$fixture/scripts/hooks/stubborn-tree.sh"
    git -C "$fixture" add scripts/hooks/stubborn-tree.sh
    git -C "$fixture" commit -qm "test: add stubborn process tree"
    token="quality-gate-${label}-$(basename "$fixture")"
    (
      cd "$fixture"
      exec "$runner" -- scripts/hooks/stubborn-tree.sh "$token" \
        >target/runner-output.json
    ) &
    runner_pid=$!
    owned_pids+=("$runner_pid")
    wait_for_file "$fixture/target/gate-ready"
    descendant="$(find_process_by_token "$token")"
    descendant_start="$(process_start_time "$descendant")"
    assert_nonempty "isolation-process-${label}-start-time" "$descendant_start"
    owned_pids+=("$descendant")
    kill -"$requested_signal" "$runner_pid"
    sleep 0.05
    kill -"$requested_signal" "$runner_pid" 2>/dev/null || true
    set +e
    timeout 10 tail --pid="$runner_pid" -f /dev/null
    code=$?
    wait "$runner_pid" 2>/dev/null
    set -e
    assert_eq "isolation-process-${label}-runner-bounded" 0 "$code"
    if ! wait_for_process_identity_gone "$descendant" "$descendant_start"; then
      _receipt_test_fail "isolation-process-${label}-descendant" gone still-running
      return 1
    fi
    assert_eq "isolation-process-${label}-receipt-count" 0 \
      "$(current_receipt_count "$fixture")"
    owned_pids=()
  done
  echo "PASS isolation-process-tree-termination"
}

run_ambient_input_isolation() {
  local fixture runner counter first second global_config excludes_file output
  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  mkdir -p "$(dirname "$fixture")/.cargo"
  printf '[build]\nrustflags=["--cfg", "ancestor_injection"]\n' \
    >"$(dirname "$fixture")/.cargo/config.toml"
  cat >"$fixture/scripts/hooks/ambient-probe.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf x >>"$1"
test ! -e ../.cargo/config.toml
test -z "${QUALITY_GATE_TEST_TOKEN:-}"
for name in config config.toml credentials credentials.toml; do
  path="${CARGO_HOME:?}/$name"
  [ ! -s "$path" ]
done
SH
  chmod +x "$fixture/scripts/hooks/ambient-probe.sh"
  git -C "$fixture" add scripts/hooks/ambient-probe.sh
  git -C "$fixture" commit -qm "test: add ambient input probe"
  first="$(cd "$fixture" && QUALITY_GATE_TEST_TOKEN=alpha \
    "$runner" -- scripts/hooks/ambient-probe.sh target/gate-counter)"
  second="$(cd "$fixture" && QUALITY_GATE_TEST_TOKEN=beta \
    "$runner" -- scripts/hooks/ambient-probe.sh target/gate-counter)"
  assert_eq isolation-ambient-first-status executed \
    "$(printf '%s' "$first" | json_field status)"
  assert_eq isolation-ambient-second-status reused \
    "$(printf '%s' "$second" | json_field status)"
  assert_eq isolation-ambient-gate-runs 1 "$(wc -c <"$counter")"
  assert_not_matches isolation-ambient-secret-values 'alpha|beta' "$first$second"

  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  printf 'external-secret\n' >"$test_root/external-symlink-target"
  ln -s "$test_root/external-symlink-target" "$fixture/external-input"
  cat >"$fixture/scripts/hooks/symlink-probe.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf x >>"$1"
test ! -e external-input
SH
  chmod +x "$fixture/scripts/hooks/symlink-probe.sh"
  git -C "$fixture" add external-input scripts/hooks/symlink-probe.sh
  git -C "$fixture" commit -qm "test: add external symlink probe"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/symlink-probe.sh \
    target/gate-counter)"
  assert_eq isolation-external-symlink-status executed \
    "$(printf '%s' "$output" | json_field status)"
  assert_eq isolation-external-symlink-reason provenance_invalid \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq isolation-external-symlink-receipts 0 \
    "$(current_receipt_count "$fixture")"
  assert_eq isolation-external-symlink-host external-secret \
    "$(<"$test_root/external-symlink-target")"

  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  excludes_file="$test_root/global-excludes"
  global_config="$test_root/global-gitconfig"
  printf 'globally-ignored\n' >"$excludes_file"
  git config -f "$global_config" core.excludesFile "$excludes_file"
  cat >"$fixture/scripts/hooks/dirty-gate.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf x >>"$1"
SH
  chmod +x "$fixture/scripts/hooks/dirty-gate.sh"
  git -C "$fixture" add scripts/hooks/dirty-gate.sh
  git -C "$fixture" commit -qm "test: add dirty gate"
  printf 'must-remain-dirty\n' >"$fixture/globally-ignored"
  output="$(cd "$fixture" && GIT_CONFIG_GLOBAL="$global_config" \
    "$runner" -- scripts/hooks/dirty-gate.sh target/gate-counter)"
  assert_eq isolation-global-exclude-status executed \
    "$(printf '%s' "$output" | json_field status)"
  assert_eq isolation-global-exclude-reason dirty_state \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq isolation-global-exclude-receipts 0 "$(current_receipt_count "$fixture")"
  echo "PASS isolation-ambient-inputs"
}

run_isolation_failure_paths() {
  local fixture runner counter output code
  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  (cd "$fixture" && "$runner" -- scripts/hooks/true-gate.sh \
    target/gate-counter) >/dev/null
  set +e
  output="$(cd "$fixture" && CSA_QUALITY_GATE_TEST_ISOLATION_FAILURE=missing \
    "$runner" -- scripts/hooks/true-gate.sh target/gate-counter)"
  code=$?
  set -e
  assert_eq isolation-missing-exit 125 "$code"
  assert_eq isolation-missing-status gate_failed \
    "$(printf '%s' "$output" | json_field status)"
  assert_eq isolation-missing-reason isolation_unavailable \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq isolation-missing-no-reuse 1 "$(wc -c <"$counter")"

  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/gate-counter"
  set +e
  output="$(cd "$fixture" && CSA_QUALITY_GATE_TEST_ISOLATION_FAILURE=start \
    "$runner" -- scripts/hooks/true-gate.sh target/gate-counter)"
  code=$?
  set -e
  assert_eq isolation-start-exit 125 "$code"
  assert_eq isolation-start-reason isolation_start_failed \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_path_absent isolation-start-no-gate "$counter"
  assert_eq isolation-start-no-receipt 0 "$(current_receipt_count "$fixture")"
  echo "PASS isolation-failure-paths"
}

run_parent_death_cleanup() {
  local fixture runner token runner_pid descendant descendant_start output
  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  cat >"$fixture/scripts/hooks/delayed-writer.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
trap '' HUP INT TERM
setsid bash -c 'trap "" HUP INT TERM; sleep 1; printf delayed >target/delayed-writer; mkdir -p .csa/state/quality-gate-receipts; printf forged >.csa/state/quality-gate-receipts/forged.json; while :; do sleep 1; done' "$1" &
printf ready >target/parent-death-ready
wait
SH
  chmod +x "$fixture/scripts/hooks/delayed-writer.sh"
  git -C "$fixture" add scripts/hooks/delayed-writer.sh
  git -C "$fixture" commit -qm "test: add delayed writer"
  token="quality-gate-parent-death-$(basename "$fixture")"
  (
    cd "$fixture"
    exec "$runner" -- scripts/hooks/delayed-writer.sh "$token" \
      >target/parent-death-output.json
  ) &
  runner_pid=$!
  owned_pids+=("$runner_pid")
  wait_for_file "$fixture/target/parent-death-ready"
  descendant="$(find_process_by_token "$token")"
  descendant_start="$(process_start_time "$descendant")"
  assert_nonempty isolation-parent-death-start-time "$descendant_start"
  owned_pids+=("$descendant")
  kill -KILL "$runner_pid"
  wait "$runner_pid" 2>/dev/null || true
  if ! wait_for_process_identity_gone "$descendant" "$descendant_start"; then
    _receipt_test_fail isolation-parent-death-descendant gone still-running
    return 1
  fi
  owned_pids=()
  sleep 1.1
  assert_path_absent isolation-parent-death-delayed-writer \
    "$fixture/target/delayed-writer"
  assert_eq isolation-parent-death-no-receipt 0 "$(current_receipt_count "$fixture")"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/true-gate.sh \
    target/gate-counter)"
  assert_eq isolation-parent-death-lock-reacquired executed \
    "$(printf '%s' "$output" | json_field status)"

  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  cat >"$fixture/scripts/hooks/early-exit.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
setsid bash -c 'trap "" HUP INT TERM; sleep 1; printf delayed >target/early-exit-writer' &
exit 0
SH
  chmod +x "$fixture/scripts/hooks/early-exit.sh"
  git -C "$fixture" add scripts/hooks/early-exit.sh
  git -C "$fixture" commit -qm "test: add early-exit descendant"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/early-exit.sh)"
  assert_eq isolation-early-exit-status executed \
    "$(printf '%s' "$output" | json_field status)"
  sleep 1.1
  assert_path_absent isolation-early-exit-delayed-writer \
    "$fixture/target/early-exit-writer"
  assert_eq isolation-early-exit-receipt 1 "$(current_receipt_count "$fixture")"
  echo "PASS isolation-parent-death-cleanup"
}

case "$scenario" in
  forgery)
    receipt_contract_set_case digest-correct-forgery
    run_digest_correct_forgery
    ;;
  state-capabilities)
    receipt_contract_set_case state-capabilities
    run_state_capability_isolation
    ;;
  process-tree)
    receipt_contract_set_case process-tree
    run_process_tree_termination
    ;;
  ambient-inputs)
    receipt_contract_set_case ambient-inputs
    run_ambient_input_isolation
    ;;
  offline-toolchain)
    receipt_contract_set_case offline-toolchain
    run_offline_pinned_toolchain
    ;;
  isolation-failure)
    receipt_contract_set_case isolation-failure
    run_isolation_failure_paths
    ;;
  parent-death)
    receipt_contract_set_case parent-death
    run_parent_death_cleanup
    ;;
  all)
    receipt_contract_set_case digest-correct-forgery
    run_digest_correct_forgery
    receipt_contract_set_case state-capabilities
    run_state_capability_isolation
    receipt_contract_set_case process-tree
    run_process_tree_termination
    receipt_contract_set_case ambient-inputs
    run_ambient_input_isolation
    receipt_contract_set_case offline-toolchain
    run_offline_pinned_toolchain
    receipt_contract_set_case isolation-failure
    run_isolation_failure_paths
    receipt_contract_set_case parent-death
    run_parent_death_cleanup
    ;;
  *) echo "unknown scenario: $scenario" >&2; exit 2 ;;
esac
