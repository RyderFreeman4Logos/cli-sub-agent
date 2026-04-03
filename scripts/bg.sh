#!/bin/bash
# bg.sh — Launch a command as a true background process immune to parent death.
#
# Claude Code (and similar LLM CLI harnesses) intercept `nohup ... &` and wrap
# them in their own process management. This script appears as a *foreground*
# process to the harness, but internally launches the real command via nohup,
# verifies it is alive, then exits — leaving the child running independently.
#
# Usage: scripts/bg.sh <logfile> <command...>
#
# Examples:
#   scripts/bg.sh /tmp/build.log cargo build --release
#   scripts/bg.sh /tmp/pre-commit.log just pre-commit
#   scripts/bg.sh /tmp/csa.log csa run --sa-mode true --tier tier-1 "task"
set -euo pipefail

if [ $# -lt 2 ]; then
  echo "Usage: bg.sh <logfile> <command...>" >&2
  exit 1
fi

LOGFILE="$1"; shift
mkdir -p "$(dirname "$LOGFILE")"
export LOGFILE

# Launch with nohup, fully detached, and persist the exit code for cross-shell polling.
nohup bash -c '"$@"; echo $? > "${LOGFILE}.exitcode"' _ "$@" >> "$LOGFILE" 2>&1 &
PID=$!
echo "PID=$PID LOG=$LOGFILE"

# Wait briefly and verify the child is alive
sleep 3
if kill -0 "$PID" 2>/dev/null; then
  echo "ALIVE pid=$PID"
  exit 0
else
  echo "DEAD pid=$PID — last 20 lines:" >&2
  tail -20 "$LOGFILE" >&2
  exit 1
fi
