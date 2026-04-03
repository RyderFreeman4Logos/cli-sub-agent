---
name: nohup-poll
description: "Launch long-running commands (>4 min) via nohup and poll every 245s to keep KV cache warm. Saves 95%+ token cost vs letting cache expire."
allowed-tools: Bash, Read, Glob
---

# Nohup Poll: KV Cache-Warm Long-Running Execution

Launch long-running commands as true background processes and poll at 245-second
intervals. Each poll is a **separate Bash tool call** that triggers a new API
request, keeping the provider's KV cache warm.

## Why This Matters

API providers cache the input token prefix with a ~5 minute TTL:

| Event | Cost |
|-------|------|
| Cache HIT (prefix reused within TTL) | **1/12.5** of miss price |
| Cache MISS (TTL expired or first call) | Full price |

A 10-minute blocking Bash call creates a >5 min gap between API calls.
The entire context (100K+ tokens) is re-read at **full miss price**.

**One cold restart on a 100K context ≈ 2500 cache-hit polling rounds.**

For a typical 15-minute `cargo build`:
- Blocking: 1 cold restart = 100K tokens × miss_price
- Polling: ~4 rounds × ~500 tokens × hit_price = 2000 × (miss_price / 12.5) = **160 × miss_price**
- **Savings: 99.8%**

## When to Activate

**MUST use** for any command expected to exceed **4 minutes**:

| Command | Typical Duration | Polls (~245s each) |
|---------|-----------------|-------------------|
| `cargo build` (full, large project) | 5–15 min | 1–4 |
| `just pre-commit` (with e2e tests) | 5–20 min | 1–5 |
| `cargo install --locked <large>` | 10–30 min | 2–7 |
| `csa run` / `csa review` / `csa debate` | 5–60 min | 1–15 |
| Data processing / rsync | varies | varies |

**Do NOT use** for commands under 4 minutes — the overhead isn't worth it.

## Protocol

### Step 1: Ensure bg.sh Exists

Look for `scripts/bg.sh` in the project root. If missing, create it:

```bash
mkdir -p scripts && cat > scripts/bg.sh << 'BGEOF'
#!/bin/bash
set -euo pipefail
if [ $# -lt 2 ]; then echo "Usage: bg.sh <logfile> <command...>" >&2; exit 1; fi
LOGFILE="$1"; shift; mkdir -p "$(dirname "$LOGFILE")"
nohup "$@" >> "$LOGFILE" 2>&1 &
PID=$!; echo "PID=$PID LOG=$LOGFILE"
sleep 3
if kill -0 "$PID" 2>/dev/null; then echo "ALIVE pid=$PID"; exit 0
else echo "DEAD pid=$PID" >&2; tail -20 "$LOGFILE" >&2; exit 1; fi
BGEOF
chmod +x scripts/bg.sh
```

### Step 2: Launch

```bash
bash scripts/bg.sh /tmp/<task>-$(date +%s).log <command...>
```

Parse the output to extract `PID` and `LOG` path. If output says `DEAD`, the
command failed immediately — read the log and stop.

### Step 3: Poll Loop

**CRITICAL: Each poll MUST be a separate Bash tool call.** A `while sleep`
loop inside one Bash call defeats the purpose — the entire loop is one API
gap and the cache expires.

**Single poll command (copy-paste ready):**

```bash
sleep 245 && if kill -0 <PID> 2>/dev/null; then echo "POLL:RUNNING"; tail -3 <LOG>; else echo "POLL:DONE exit=$(wait <PID> 2>/dev/null; echo $?)"; tail -20 <LOG>; fi
```

Set Bash timeout to **300000** ms (300s = 245s sleep + 55s margin).

**After each poll, decide:**

| Output contains | Action |
|----------------|--------|
| `POLL:RUNNING` | Issue another identical poll (new Bash call) |
| `POLL:DONE exit=0` | Success — proceed with workflow |
| `POLL:DONE exit=<N>` | Failure — read full log, diagnose |

### Step 4: Completion

When process finishes, read the full log:

```bash
cat <LOG>
```

Or tail a reasonable amount if the log is large:

```bash
tail -100 <LOG>
```

## Interleaving with Other Work

If you have **independent work** to do while waiting (read-only research,
documentation, etc.), you can interleave:

1. Launch the long command via Step 2
2. Do other work for a while
3. Before 245s pass, issue a quick status check:
   ```bash
   kill -0 <PID> 2>/dev/null && echo "STILL_RUNNING" || echo "DONE"
   ```
4. Continue alternating work and checks, staying under the 5-min TTL

The rule: **never let >4 minutes pass without an API interaction** (any tool
call counts — Read, Grep, Bash, etc.).

## Cache TTL by Provider

| Provider | Approximate TTL | Safe Poll Interval |
|----------|----------------|-------------------|
| Anthropic (Claude) | 5 min (300s) | **245s** |
| OpenAI (Codex) | 5 min (300s) | **245s** |
| Google (Gemini) | 5 min (300s) | **245s** |

245 seconds leaves a 55-second safety margin for API round-trip latency.

## Accumulated Poll Cost

Each poll adds ~200–500 tokens of history. After N polls:

| Polls | Duration | Accumulated Tokens | Cost vs Cold Restart (100K ctx) |
|-------|----------|-------------------|-------------------------------|
| 5 | ~20 min | ~2.5K | **0.2%** |
| 20 | ~80 min | ~10K | **0.8%** |
| 100 | ~7 hours | ~50K | **4%** |
| 350 | ~1 day | ~175K | **14%** |

Even polling continuously for a full day costs only ~14% of one cold restart.
The break-even point is ~12–18 days of continuous polling.

## Examples

### Cargo Build

```bash
# Step 1: Launch
bash scripts/bg.sh /tmp/cargo-build-$(date +%s).log cargo build --release
# → PID=12345 LOG=/tmp/cargo-build-1743666000.log

# Step 2: First poll (separate Bash call, timeout 300000ms)
sleep 245 && if kill -0 12345 2>/dev/null; then echo "POLL:RUNNING"; tail -3 /tmp/cargo-build-1743666000.log; else echo "POLL:DONE exit=$(wait 12345 2>/dev/null; echo $?)"; tail -20 /tmp/cargo-build-1743666000.log; fi

# Step 3: Repeat until POLL:DONE
```

### CSA Run

```bash
# Step 1: Launch
bash scripts/bg.sh /tmp/csa-run-$(date +%s).log csa run --sa-mode true --tier tier-1 "Implement feature X"
# → PID=23456 LOG=/tmp/csa-run-1743666000.log

# Step 2: Poll (can also check session status)
sleep 245 && if kill -0 23456 2>/dev/null; then echo "POLL:RUNNING"; tail -3 /tmp/csa-run-1743666000.log; else echo "POLL:DONE exit=$(wait 23456 2>/dev/null; echo $?)"; tail -20 /tmp/csa-run-1743666000.log; fi
```

### Just Pre-commit

```bash
# Step 1: Launch
bash scripts/bg.sh /tmp/precommit-$(date +%s).log just pre-commit
# → PID=34567 LOG=/tmp/precommit-1743666000.log

# Step 2: Poll
sleep 245 && if kill -0 34567 2>/dev/null; then echo "POLL:RUNNING"; tail -3 /tmp/precommit-1743666000.log; else echo "POLL:DONE exit=$(wait 34567 2>/dev/null; echo $?)"; tail -20 /tmp/precommit-1743666000.log; fi
```

## Anti-Patterns

| Wrong | Why | Correct |
|-------|-----|---------|
| `while sleep 245; do check; done` in one Bash call | Single API gap — cache expires | Separate Bash call per poll |
| `sleep 400 && check` | 400s > 300s TTL — cache expires | `sleep 245` (under TTL) |
| `run_in_background: true` then forget | No periodic API calls — cache expires | Poll or do other work |
| Blocking Bash with 600s timeout | 600s > TTL — cache expires | Use nohup-poll instead |
