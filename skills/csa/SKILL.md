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
2. **`--tier <name>`** — canonical selector whenever project `[tiers]` is non-empty.
3. **`--tool <tool>`** — only a soft try-first preference inside the selected tier. `[review].tool` and `[debate].tool` behave the same way; they no longer hard-whitelist the tier.
4. **Direct model or force bypass** — `--model-spec`, `--force-ignore-tier-setting`/`--force-tier`, and broad direct-routing force flags are emergency-only under configured tiers. They are rejected unless `[tier_policy].allow_force_bypass = true` is set in the global config, or CSA is continuing an already-trusted inherited #1741 subtree pin. Project `.csa/config.toml` cannot grant this.

**Priority chain**: inherited trusted pin > `--tier` > config tier / tier mapping > soft tool preference > auto-select. Direct bypass is outside the normal chain and requires the global escape hatch.

### Quick Reference

```bash
# When tiers ARE configured (most CSA projects):
# Use tier names from your .csa/config.toml [tiers] section
csa run --sa-mode true --tier <tier-name> "Implement feature X"
csa review --sa-mode true --tier <tier-name> --tool codex --range main...HEAD
csa debate --sa-mode true --tier <tier-name> --tool claude-code "REST vs gRPC?"

# Emergency exact model only when global [tier_policy].allow_force_bypass=true
# or when continuing an inherited trusted subtree pin:
csa run --sa-mode true --force-ignore-tier-setting --model-spec codex/openai/gpt-5.4/xhigh "Emergency pinned run"

# When tiers are NOT configured (legacy / simple projects):
csa run --sa-mode true --tool codex "Implement feature X"
```

### Canonical LLM dispatch form

Prefer this form for general `csa` dispatch when tiers are configured:

```bash
csa run \
  --sa-mode true \
  --tier <tier-name> \
  --timeout 7200 \
  --prompt-file /path/to/prompt.md
```

Add `--tool <tool>` only when you want try-first ordering inside that tier:

```bash
csa run \
  --sa-mode true \
  --tier <tier-name> \
  --tool codex \
  --timeout 7200 \
  --prompt-file /path/to/prompt.md
```

Why this is the canonical LLM-friendly form:

- `--tier` preserves the configured quality/cost policy and fallback chain
- `--tool` is optional try-first ordering, not a brittle hard filter
- `--timeout 7200` is the sprint-safe default

Default to tier-based routing. Use direct `--model-spec` only for the global
escape hatch or an inherited trusted subtree pin.

## Prompt Crafting (How to Write Better CSA Prompts)

Full guide: `references/prompt-crafting-guide.md` (load on demand).

**Quick checklist for every CSA dispatch prompt:**

1. **Objective**: State what to accomplish in 1-2 sentences (not "fix bugs")
2. **Context**: Issue description, file paths, prior findings (do NOT pre-fetch file contents)
3. **Boundaries**: What to change vs what to leave alone; negative constraints as guardrails
4. **Output format**: Commit conventions, expected deliverables
5. **Verification**: "Run just pre-commit before committing" or equivalent check

**Three agentic anchors** (add to every prompt for ~20% task completion improvement):
- Persistence: "Keep working until fully resolved"
- Tool-calling: "Use tools to verify — do not guess"
- Planning: "Plan before each action, reflect after each result"

**Key principles:**
- Positive instructions first, negative constraints as guardrails
- Match style to model: prescriptive for Codex, outcome-oriented for Claude
- Do NOT add "think step by step" for reasoning models (o1/o3) — redundant and harmful
- 3 diverse examples > 20 edge-case bullet points
- Critical constraints need defense in depth: prompt + sandbox + verification

## When NOT to Use CSA

CSA session spawn has a fixed cold-start cost (~10K-60K tokens for rules/context ingestion).
For small tasks, the cold-start cost can exceed the actual work cost by 10-20x.

**Prefer native subagent (Agent tool) or direct execution when**:
- Change is ≤30 lines across ≤3 files
- Same model as main agent (cache sharing via native Agent)
- No filesystem sandbox or resource isolation needed
- No cross-tool heterogeneous review needed (e.g., codex write + gemini review)

**Use CSA when**:
- Cross-tool execution (different model/provider than main agent)
- Filesystem sandbox isolation required (bwrap/landlock)
- Memory/PID resource limits needed (cgroup)
- Long-running task (>1h, may outlive main agent session)
- Audit trail required (session metadata, review verdict, token tracking)
- Task exceeds 100 lines or 5+ files

## Core Concepts

- **Meta-Session**: Persistent workspace for a specific task, stored in `~/.local/state/csa/`.
- **Recursion**: Agents can spawn sub-agents by calling `csa` again. Depth is tracked and limited.
- **Genealogy**: All sessions maintain parent-child relationships, forming a tree.
- **Model Spec**: Unified format `tool/provider/model/thinking_budget`.
- **Resource Guard**: Pre-flight memory check prevents OOM when launching tools.

## Review/Debate Discipline (MANDATORY)

When the task is specifically review or debate:

1. Use the built-in subcommand that matches the intent.
   - Review task -> `csa review`
   - Debate task -> `csa debate`
2. Do **NOT** replace `csa review` / `csa debate` with a hand-written `csa run`
   prompt unless the built-in command is blocked by a concrete, documented error.
3. In slow Rust repositories, one healthy review/debate session taking 30-60
   minutes is normal.
4. Sparse early output or a `csa session wait` timeout is **not** evidence of
   failure by itself.
5. If the session is still healthy, keep waiting on the **same session id**.
   Do **NOT** launch narrowed or duplicate review/debate sessions for the same
   scope.
6. Fallback to a second session only when there is strong evidence of failure:
   explicit crash/error, persistent liveness failure, or explicit user
   instruction.

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
