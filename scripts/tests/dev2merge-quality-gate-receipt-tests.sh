#!/usr/bin/env bash
set -euo pipefail
shopt -s inherit_errexit
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
export GIT_CONFIG_NOSYSTEM=1

repo_root="$(git rev-parse --show-toplevel)"
source "$repo_root/scripts/tests/quality-gate-test-assertions.sh"
receipt_contract_install_failure_trap dev2merge-quality-gate-receipt-tests.sh
receipt_contract_set_case source-contract
mkdir -p "$repo_root/drafts"
test_root="$(realpath -e "$(mktemp -d "$repo_root/drafts/dev2merge-quality-gate.XXXXXX")")"
trap 'rm -rf -- "$test_root"' EXIT
step_eleven="$(python3 - "$repo_root/patterns/dev2merge/workflow.toml" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as source:
    workflow = tomllib.load(source)
for step in workflow["workflow"]["steps"]:
    if step["title"] == "Self-Review Gate":
        print(step["prompt"])
        break
PY
)"
assert_contains dev2merge-workflow-quality-gate 'just quality-gates' "$step_eleven"
assert_contains dev2merge-workflow-cargo-fallback 'elif [ -f Cargo.toml ]' "$step_eleven"
step_eleven_script="$test_root/step-eleven.sh"
python3 - "$repo_root/patterns/dev2merge/workflow.toml" >"$step_eleven_script" <<'PY'
import re
import sys
import tomllib

with open(sys.argv[1], "rb") as source:
    workflow = tomllib.load(source)
prompt = next(
    step["prompt"]
    for step in workflow["workflow"]["steps"]
    if step["title"] == "Self-Review Gate"
)
match = re.search(r"```bash\n(.*?)```", prompt, flags=re.DOTALL)
if match is None:
    raise SystemExit("Self-Review Gate has no Bash block")
print(match.group(1), end="")
PY
chmod +x "$step_eleven_script"

run_step_eleven_runtime_matrix() {
  local case_root fake_bin output code
  case_root="$test_root/step-summary-failure"
  fake_bin="$case_root/bin"
  mkdir -p "$fake_bin"
  printf 'quality-gates:\n' >"$case_root/justfile"
  printf '#!/usr/bin/env bash\nexit 69\n' >"$fake_bin/just"
  chmod +x "$fake_bin/just"
  set +e
  output="$(cd "$case_root" && PATH="$fake_bin:/usr/bin:/bin" \
    bash "$step_eleven_script" 2>&1)"
  code=$?
  set -e
  assert_ne step-eleven-summary-failure-exit 0 "$code"
  assert_contains step-eleven-summary-failure-diagnostic \
    'just --summary failed' "$output"

  case_root="$test_root/step-quality-recipe"
  fake_bin="$case_root/bin"
  mkdir -p "$fake_bin"
  printf 'quality-gates:\n' >"$case_root/justfile"
  cat >"$fake_bin/just" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = --summary ]; then
  printf 'quality-gates\n'
elif [ "${1:-}" = quality-gates ]; then
  printf x >>"$STEP_COUNTER"
else
  exit 64
fi
SH
  chmod +x "$fake_bin/just"
  (cd "$case_root" && STEP_COUNTER="$case_root/count" \
    PATH="$fake_bin:/usr/bin:/bin" bash "$step_eleven_script")
  assert_eq step-eleven-quality-recipe-runs 1 "$(wc -c <"$case_root/count")"

  case_root="$test_root/step-python-missing"
  mkdir -p "$case_root/empty-bin"
  printf '[project]\nname="fixture"\n' >"$case_root/pyproject.toml"
  set +e
  output="$(cd "$case_root" && PATH="$case_root/empty-bin:/usr/bin:/bin" \
    bash "$step_eleven_script" 2>&1)"
  code=$?
  set -e
  assert_ne step-eleven-python-missing-exit 0 "$code"
  assert_contains step-eleven-python-missing-diagnostic \
    'Python project has no usable lint runner' "$output"

  case_root="$test_root/step-python-runners"
  fake_bin="$case_root/bin"
  mkdir -p "$fake_bin"
  printf '[project]\nname="fixture"\n' >"$case_root/pyproject.toml"
  for runner in ruff pytest; do
    printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>"$STEP_COUNTER"\n' \
      >"$fake_bin/$runner"
    chmod +x "$fake_bin/$runner"
  done
  (cd "$case_root" && STEP_COUNTER="$case_root/count" \
    PATH="$fake_bin:/usr/bin:/bin" bash "$step_eleven_script")
  assert_eq step-eleven-python-runner-calls 3 "$(wc -c <"$case_root/count")"

  case_root="$test_root/step-javascript-missing"
  mkdir -p "$case_root/empty-bin"
  printf '{}\n' >"$case_root/package.json"
  set +e
  output="$(cd "$case_root" && PATH="$case_root/empty-bin:/usr/bin:/bin" \
    bash "$step_eleven_script" 2>&1)"
  code=$?
  set -e
  assert_ne step-eleven-javascript-missing-exit 0 "$code"
  assert_contains step-eleven-javascript-missing-diagnostic \
    'JavaScript project has no usable lint runner' "$output"
  echo "PASS dev2merge-step-eleven-runtime-matrix"
}

receipt_contract_set_case step-eleven-runtime-matrix
run_step_eleven_runtime_matrix

hook_receipt_field() {
  local field="$1"
  python3 -c '
import json,sys
field=sys.argv[1]
for line in sys.stdin:
    try: value=json.loads(line.strip())
    except json.JSONDecodeError: continue
    if "receipt_identity" in value and field in value:
        print(value[field])
        raise SystemExit(0)
print(f"FAIL hook-receipt-{field} expected=field-present actual=field-missing", file=sys.stderr)
raise SystemExit(1)
' "$field"
}

receipt_contract_set_case dev2merge-quality-gate-receipt
fixture="$test_root/repo"
mkdir -p "$fixture/scripts/hooks" "$fixture/scripts" "$fixture/.csa/state" \
  "$fixture/target/quality-gate-test-state"
printf '/.csa/state/\n/target/\n' >"$fixture/.gitignore"
git -C "$fixture" init -q
git -C "$fixture" config user.name "Dev2merge Tests"
git -C "$fixture" config user.email "dev2merge-tests@example.invalid"
git -C "$fixture" remote add origin https://example.invalid/dev2merge.git
cp "$repo_root/scripts/hooks/quality-gate-receipt.sh" "$fixture/scripts/hooks/"
cp "$repo_root/scripts/hooks/quality-gates.sh" "$fixture/scripts/hooks/"
cp "$repo_root/scripts/cargo-env-normalize.sh" "$fixture/scripts/"
cp "$repo_root/scripts/quality-gate-state.py" "$fixture/scripts/"
cp "$repo_root/scripts/quality_gate_secure_state.py" "$fixture/scripts/"
cp "$repo_root/scripts/quality_gate_provenance.py" "$fixture/scripts/"
cp "$repo_root/scripts/quality_gate_sandbox.py" "$fixture/scripts/"
cp "$repo_root/scripts/quality_gate_host_attestation.py" "$fixture/scripts/"
cp "$repo_root/scripts/quality_gate_process.py" "$fixture/scripts/"
cp "$repo_root/scripts/quality_gate_environment.py" "$fixture/scripts/"
cp "$repo_root/scripts/quality_gate_toolchain.py" "$fixture/scripts/"
cp "$repo_root/scripts/rename-no-replace.py" "$fixture/scripts/"
cp "$repo_root/rust-toolchain.toml" "$fixture/"
printf '[workspace]\n' >"$fixture/Cargo.toml"
printf '# lock\n' >"$fixture/Cargo.lock"
printf '# weave\n' >"$fixture/weave.lock"
printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>target/quality-gate-test-state/quality-counter\n' \
  >"$fixture/scripts/hooks/pre-push-quality-gates.sh"
printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>target/quality-gate-test-state/live-counter\n' \
  >"$fixture/scripts/hooks/quality-gates-live.sh"
for gate in branch-protection version-check review-check; do
  printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>target/quality-gate-test-state/%s-counter\n' "$gate" \
    >"$fixture/scripts/hooks/${gate}.sh"
done
chmod +x "$fixture/scripts/hooks/"*.sh
cat >"$fixture/justfile" <<'EOF'
quality-gates:
    @scripts/hooks/quality-gates.sh

pre-push:
    @CSA_QUALITY_GATE_HOOK_MODE=1 scripts/hooks/quality-gates.sh
EOF
cp "$repo_root/lefthook.yml" "$fixture/lefthook.yml"
git -C "$fixture" add .gitignore Cargo.toml Cargo.lock weave.lock justfile \
  lefthook.yml rust-toolchain.toml scripts
git -C "$fixture" commit -qm "test: initialize dev2merge fixture"

producer_started_ns="$(date +%s%N)"
producer="$(cd "$fixture" && just quality-gates)"
producer_elapsed_ms="$(( ($(date +%s%N) - producer_started_ns) / 1000000 ))"
producer_status="$(hook_receipt_field status <<<"$producer")"
producer_identity="$(hook_receipt_field receipt_identity <<<"$producer")"
assert_eq dev2merge-producer-status executed "$producer_status"
(cd "$fixture" && scripts/hooks/review-check.sh)
consumer_started_ns="$(date +%s%N)"
consumer="$(cd "$fixture" && lefthook run pre-push 2>&1)"
consumer_elapsed_ms="$(( ($(date +%s%N) - consumer_started_ns) / 1000000 ))"
consumer_status="$(hook_receipt_field status <<<"$consumer")"
consumer_identity="$(hook_receipt_field receipt_identity <<<"$consumer")"

assert_eq dev2merge-consumer-status reused "$consumer_status"
assert_eq dev2merge-consumer-identity "$producer_identity" "$consumer_identity"
assert_eq dev2merge-reuse-quality-runs 1 "$(wc -c <"$fixture/target/quality-gate-test-state/quality-counter")"
assert_eq dev2merge-reuse-live-runs 2 \
  "$(wc -c <"$fixture/target/quality-gate-test-state/live-counter")"
assert_eq dev2merge-reuse-branch-protection-runs 1 \
  "$(wc -c <"$fixture/target/quality-gate-test-state/branch-protection-counter")"
assert_eq dev2merge-reuse-version-check-runs 1 \
  "$(wc -c <"$fixture/target/quality-gate-test-state/version-check-counter")"
assert_eq dev2merge-reuse-review-check-runs 2 \
  "$(wc -c <"$fixture/target/quality-gate-test-state/review-check-counter")"

echo "PASS dev2merge-quality-gate-receipt identity=${producer_identity} quality_runs=1 executed_ms=${producer_elapsed_ms} reused_ms=${consumer_elapsed_ms}"
