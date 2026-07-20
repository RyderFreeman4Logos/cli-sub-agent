# shellcheck shell=bash
# Environment, provenance, and process-boundary contracts for isolation tests.
# Sourced after the isolation fixture and assertion helpers are defined.

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  echo 'source-only helper; run: bash scripts/tests/quality-gate-isolation-tests.sh ambient-inputs' >&2
  exit 2
fi

run_python_boundary_contracts() {
  local code output

  output="$(PYTHONDONTWRITEBYTECODE=1 python3 - "$repo_root/scripts" <<'PY'
import sys

sys.path.insert(0, sys.argv[1])
from quality_gate_environment import normalized_static_environment

volatile = {"CARGO_MAKEFLAGS", "CARGO_TARGET_TMPDIR", "RUST_RECURSION_COUNT"}
normalized = normalized_static_environment(
    {name: "host-owned" for name in volatile},
    "a" * 64,
    "b" * 64,
    ("true", "true", "c" * 64, "d" * 40),
)
print(",".join(sorted(volatile & normalized.keys())))
PY
)"
  assert_empty isolation-volatile-cargo-env-dropped "$output"

  set +e
  output="$(PYTHONDONTWRITEBYTECODE=1 python3 - "$repo_root/scripts" "$repo_root" 2>&1 <<'PY'
import os
import sys
from pathlib import Path

sys.path.insert(0, sys.argv[1])
from quality_gate_provenance import ProvenanceError, tool_provenance

environment = os.environ.copy()
environment["CC"] = "quality-gate-definitely-missing-bare-cc"
try:
    tool_provenance(Path(sys.argv[2]), environment)
except ProvenanceError:
    raise SystemExit(0)
raise SystemExit("unresolved bare CC was accepted")
PY
)"
  code=$?
  set -e
  assert_eq isolation-bare-cc-fails-closed 0 "$code"

  set +e
  output="$(timeout 3 env PYTHONDONTWRITEBYTECODE=1 python3 - "$repo_root/scripts" 2>&1 <<'PY'
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, sys.argv[1])
import quality_gate_sandbox

with tempfile.TemporaryDirectory() as raw_root:
    root = Path(raw_root)
    regular = root / "regular"
    regular.write_bytes(b"regular")
    fifo = root / "tracked-fifo"
    os.mkfifo(fifo)
    regular_status = os.lstat(regular)
    original_lstat = quality_gate_sandbox.os.lstat
    quality_gate_sandbox.os.lstat = lambda path: (
        regular_status if Path(path) == fifo else original_lstat(path)
    )
    try:
        assert quality_gate_sandbox._read_tracked_value(
            root, "100644", "tracked-fifo"
        ) is None
    finally:
        quality_gate_sandbox.os.lstat = original_lstat
PY
)"
  code=$?
  set -e
  assert_eq isolation-tracked-fifo-read-bounded 0 "$code"
  echo "PASS isolation-python-boundary-contracts"
}

run_process_wall_clock_deadline() {
  local code output
  set +e
  output="$(timeout 5 env PYTHONDONTWRITEBYTECODE=1 python3 - \
    "$repo_root/scripts" 2>&1 <<'PY'
import sys
from pathlib import Path

sys.path.insert(0, sys.argv[1])
import quality_gate_process

quality_gate_process.EXECUTION_TIMEOUT_SECONDS = 0.2
quality_gate_process.TERM_GRACE_SECONDS = 0.1
quality_gate_process.KILL_GRACE_SECONDS = 1.0
result = quality_gate_process.execute_supervised(
    ("/bin/sh", "-c", "trap '' TERM; while :; do :; done"),
    Path.cwd(),
)
if result.code != 124 or result.reason != "gate_timeout":
    raise SystemExit(f"unexpected result: {result!r}")
PY
)"
  code=$?
  set -e
  if [ "$code" -ne 0 ]; then
    printf '%s\n' "$output" >&2
  fi
  assert_eq isolation-process-wall-clock-deadline 0 "$code"
  echo "PASS isolation-process-wall-clock-deadline"
}
