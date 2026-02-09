---
name: csa
description: Unified CLI interface for executing coding tasks across multiple AI tools with persistent sessions, recursive agent spawning, and resource-aware scheduling
allowed-tools: Bash, Read, Grep, Glob
---

# CSA: CLI Sub-Agent (Recursive Agent Container)

Unified CLI interface for executing coding tasks across multiple AI tools
with persistent sessions, recursive agent spawning, and resource-aware scheduling.

## Supported Tools

| Tool | Command | Compress | Yolo |
|------|---------|----------|------|
| opencode | `csa run --tool opencode` | `/compact` | auto |
| codex | `csa run --tool codex` | `/compact` | auto |
| claude-code | `csa run --tool claude-code` | `/compact` | auto |

All tools run in yolo mode by default (auto-approve all actions).

## Core Concepts

- **Meta-Session**: Persistent workspace for a specific task, stored in `~/.local/state/csa/`.
- **Recursion**: Agents can spawn sub-agents by calling `csa` again. Depth is tracked and limited.
- **Genealogy**: All sessions maintain parent-child relationships, forming a tree.
- **Model Spec**: Unified format `tool/provider/model/thinking_budget`.
- **Resource Guard**: Pre-flight memory check prevents OOM when launching tools.

## Basic Usage

### Initialize Project
```bash
csa init --non-interactive
# Creates .csa/config.toml with detected tools
```

### Execute Tasks
```bash
# Analysis (read-only)
csa run "Analyze the authentication flow"

# Implementation (write, use opencode/codex/claude-code)
csa run --tool opencode --session my-task "Fix the login bug"

# Resume existing session
csa run --tool opencode --session 01JK... "Continue the refactor"

# Override model
csa run --tool opencode --model "provider/model-name" "Implement feature X"

# Ephemeral session (no project context, auto-cleanup)
csa run --ephemeral "What is the CAP theorem?"
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
csa run --tool opencode --parent $CSA_SESSION_ID \
  "Research PostgreSQL extensions"
```

Max recursion depth is configurable (default: 5).

### Parallel Execution (Read-Only SAFE)

Multiple analysis tasks can run in parallel safely:
```bash
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
# Step 1: Parallel research (read-only)
csa run --session research-1 "Research A" &
csa run --session research-2 "Research B" &
wait

# Step 2: Serial implementation (write)
csa run --tool opencode --session impl-1 "Implement A based on research"
csa run --tool opencode --session impl-2 "Implement B based on research"
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
If you see "OOM Risk Prevention":
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
