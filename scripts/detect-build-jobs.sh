#!/usr/bin/env bash
# Auto-detect safe CARGO_BUILD_JOBS based on available memory.
# Uses 1536MB per compile job (conservative for large Rust crates with proc macros).
# Clamped to [1, min(nproc, 8)].
set -euo pipefail

mem_mb=$(awk '/MemAvailable/ {printf "%d", $2/1024}' /proc/meminfo 2>/dev/null || echo "0")
cpus=$(nproc 2>/dev/null || echo "1")

# Guard against empty/zero values.
if [ -z "$mem_mb" ] || [ "$mem_mb" -le 0 ]; then mem_mb=1536; fi
if [ -z "$cpus" ] || [ "$cpus" -le 0 ]; then cpus=1; fi

# 1536MB per compile job.
mem_jobs=$(( mem_mb / 1536 ))

# Clamp: at least 1, at most min(cpus, 8).
if [ "$mem_jobs" -lt 1 ]; then mem_jobs=1; fi
if [ "$mem_jobs" -gt "$cpus" ]; then mem_jobs=$cpus; fi
if [ "$mem_jobs" -gt 8 ]; then mem_jobs=8; fi

echo "$mem_jobs"
