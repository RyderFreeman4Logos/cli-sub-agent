---
name: csa
description: "Use when: executing tasks via csa run/review/debate, session mgmt"
allowed-tools: Bash, Read, Grep, Glob
---

# CSA: CLI Sub-Agent (Recursive Agent Container)

Unified CLI interface for executing coding tasks across multiple AI tools
with persistent sessions, recursive agent spawning, and resource-aware scheduling.

## Supported Tools

| Tool | Compress | Yolo |
|------|----------|------|
| opencode | `/compact` | auto |
| codex | `/compact` | auto |
| claude-code | `/compact` | auto |

All tools run in yolo mode by default (auto-approve all actions).

## Invocation Checklist (MANDATORY)

Before constructing ANY `csa run`, `csa review`, or `csa debate` command, verify:

1. **`--sa-mode true|false`** — REQUIRED at root depth (CSA_DEPTH=0). Sub-agent calls (depth > 0) inherit automatically.
2. **`--tier <name>`** — REQUIRED for `csa run` when project has `[tiers]` configured. Direct `--tool` is **blocked** by the CLI. For `csa review`/`csa debate`, `--tier` selects model; `--model`/`--thinking` overrides are still allowed.
3. **NEVER use `--tool` directly** when tiers are configured — use `--tier` instead. To bypass: `--force-ignore-tier-setting` (alias: `--force-tier`).

**Priority chain**: `--tier` > config tier > `--tool` (with force) > config tool > auto-select.

### Quick Reference

```bash
# When tiers ARE configured (most CSA projects):
# Use tier names from your .csa/config.toml [tiers] section
csa run --sa-mode true --tier <tier-name> "Implement feature X"
csa review --sa-mode true --tier <tier-name> --range main...HEAD
csa debate --sa-mode true --tier <tier-name> "REST vs gRPC?"

# Bypass tier to force a specific tool (requires --force-ignore-tier-setting):
csa run --sa-mode true --force-ignore-tier-setting --tool codex "Quick fix"

# When tiers are NOT configured (legacy / simple projects):
csa run --sa-mode true --tool codex "Implement feature X"
```

## Core Concepts

- **Meta-Session**: Persistent workspace for a specific task, stored in `~/.local/state/csa/`.
- **Recursion**: Agents can spawn sub-agents by calling `csa` again. Depth is tracked and limited.
- **Genealogy**: All sessions maintain parent-child relationships, forming a tree.
- **Model Spec**: Unified format `tool/provider/model/thinking_budget`.
- **Resource Guard**: Pre-flight memory check prevents OOM when launching tools.

## --sa-mode (REQUIRED for Root Callers)

Execution commands (`run`, `review`, `debate`, `batch`, `plan run`, `claude-sub-agent`)
**require** `--sa-mode true|false` when invoked from the top level (root depth).

- `--sa-mode true`: Enable autonomous safety — injects prompt-guard mechanisms
- `--sa-mode false`: Disable autonomous safety (interactive use)

CSA-spawned children auto-detect via `CSA_DEPTH` + `CSA_INTERNAL_INVOCATION` env vars
(both set automatically by CSA when spawning sub-processes). Manual scripts that only
set `CSA_DEPTH` without `CSA_INTERNAL_INVOCATION=1` will still get the root-caller error.

```bash
# Root caller MUST specify --sa-mode
csa run --sa-mode true "Implement feature X"
csa review --sa-mode true --range main...HEAD
csa debate --sa-mode true "REST vs gRPC?"

# Internal sub-agent call (depth > 0) — --sa-mode is optional
csa run "Sub-task Y"  # inherits from parent via CSA_DEPTH
```

**Error if omitted at root depth:**
```
Error: --sa-mode true|false is required for root callers on execution commands: command `run`
```

## Basic Usage

### Initialize Project
```bash
csa init --non-interactive
# Creates .csa/config.toml with detected tools
```

### Execute Tasks
```bash
# Analysis (read-only)
csa run --sa-mode false "Analyze the authentication flow"

# Implementation — tier-based (preferred when tiers configured)
csa run --sa-mode true --tier <tier-name> "Fix the login bug"

# Implementation — direct tool (only when NO tiers configured)
csa run --sa-mode true --tool codex "Fix the login bug"

# Resume existing session via fork
csa run --sa-mode true --tier <tier-name> --fork-from 01JK... "Continue the refactor"

# Ephemeral session (no project context, auto-cleanup)
csa run --sa-mode false --ephemeral "What is the CAP theorem?"
```

### Session Management
```bash
csa session list              # List all sessions
csa session list --tree       # Show tree hierarchy
csa session list --tool opencode  # Filter by tool
csa session delete --session 01JK...  # Delete a session
```

### Configuration
```bash
csa config show       # Display current config
csa config edit       # Open in $EDITOR
csa config validate   # Validate config file
```

## Advanced: Recursion and Parallelism

### Spawning Sub-Agents
```bash
# Sub-agent inherits depth tracking via CSA_DEPTH env var
csa run --tier <tier-name> --parent $CSA_SESSION_ID \
  "Research PostgreSQL extensions"
```

Max recursion depth is configurable (default: 5).

### Parallel Execution (Read-Only SAFE)

Multiple analysis tasks can run in parallel safely:
```bash
# Sub-agent calls (depth > 0) — --sa-mode inherited from parent
csa run --session research-db "Query database docs" &
csa run --session research-ui "Query frontend docs" &
wait
```

### Parallel Writes (DANGEROUS)

**Warning**: Parallel writes are extremely risky!

Potential issues:
1. **File conflicts**: Two agents modifying the same file
2. **Build deadlocks**: `cargo build` locks `target/` directory
3. **Commit conflicts**: Pre-commit hooks may fail
4. **Lock file conflicts**: `Cargo.lock`, `package-lock.json`, etc.

**Rules**:
- Parallel reads (analysis, search): Safe
- Parallel writes to fully isolated directories: Proceed with caution
- Parallel writes to shared modules or config: Forbidden

**Recommended pattern**:
```bash
# Step 1: Parallel research (read-only, sub-agent depth — --sa-mode inherited)
csa run --session research-1 "Research A" &
csa run --session research-2 "Research B" &
wait

# Step 2: Serial implementation (write)
csa run --sa-mode true --tier <tier-name> --session impl-1 "Implement A based on research"
csa run --sa-mode true --tier <tier-name> --session impl-2 "Implement B based on research"
```

## Error Handling

### Lock Contention
If a tool is already running in a session, you'll see:
```
Tool 'opencode' is currently locked by another process (PID 12345)
```
Wait for the other process to finish, or use a different session.

### Depth Exceeded
If you see "Max recursion depth exceeded":
- Stop recursing and execute the task directly.

### OOM Prevention
If you see "Insufficient system memory":
- Wait for running agents to finish, or close other applications.
- The resource guard uses P95 historical memory estimates to prevent OOM.

## Context Window Management

Each tool has a different context compression command:
- `opencode`, `codex`, `claude-code`: `/compact`

`csa session compress` automatically selects the correct command.

## Environment Variables

CSA sets these env vars for child processes:
- `CSA_SESSION_ID`: Current session ULID
- `CSA_DEPTH`: Current recursion depth
- `CSA_PROJECT_ROOT`: Project root path
- `CSA_PARENT_SESSION`: Parent session ULID (if spawned as sub-agent)

## Configuration

Project config lives at `.csa/config.toml`. Key sections:

- **tools**: Enable/disable tools, set restrictions
- **resources**: Memory limits, per-tool estimates
- **tiers**: Model tiers for task-based selection
- **aliases**: Shortcut names for model specs
