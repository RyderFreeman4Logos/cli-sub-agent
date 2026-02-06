# CSA — CLI Sub-Agent

> Recursive Agent Container: Standardized, composable Unix processes for LLM CLI tools.

CSA provides a unified CLI interface for executing coding tasks across multiple AI tools with persistent sessions, recursive agent spawning, and resource-aware scheduling.

## When to Use CSA

CSA is most valuable when you need capabilities **beyond** what a single AI CLI tool provides:

| Scenario | Without CSA | With CSA |
|----------|-------------|----------|
| **Multi-tool workflows** | Manually switch between gemini-cli, codex, claude-code | `csa run --tool X` with unified interface |
| **Recursive agents** | No safe way for an agent to spawn sub-agents | Depth-limited spawning with `CSA_DEPTH` |
| **Session continuity** | Each tool manages sessions differently | Unified ULID sessions with genealogy trees |
| **Resource safety** | No OOM prevention when running parallel agents | P95 memory estimation blocks unsafe launches |
| **Audit trail** | Scattered logs across tools | Session tree with logs, locks, and state |

**Example: Multi-step code review pipeline**

```bash
# Step 1: Analyze with gemini-cli (2M context, read-only)
csa run --tool gemini-cli "Analyze the auth module for security issues"

# Step 2: Fix issues with codex (in the same session tree)
csa run --tool codex --parent $CSA_SESSION_ID "Fix the XSS vulnerability found"

# Step 3: Review the fix
csa review --diff
```

If you only use a single AI tool for simple tasks, the tool's native CLI may suffice. CSA shines when orchestrating multiple tools or managing complex agent hierarchies.

## Features

- **Multi-tool support** — Seamlessly switch between gemini-cli, opencode, codex, and claude-code
- **Recursive agents** — Agents spawn sub-agents forming execution trees with depth limiting
- **Resource guard** — P95 memory estimation prevents OOM when launching parallel agents
- **Session management** — Persistent sessions with ULID IDs, genealogy tracking, and tree visualization
- **Unified model spec** — `tool/provider/model/thinking_budget` format across all tools
- **Safety controls** — Tool-level file locking, edit restrictions, signal propagation to child processes

## Installation

### Quick Install (macOS / Linux)

```bash
curl -sSf https://raw.githubusercontent.com/RyderFreeman4Logos/cli-sub-agent/main/install.sh | bash
```

### Manual Install

Requires [Rust toolchain](https://rustup.rs/):

```bash
cargo install --git https://github.com/RyderFreeman4Logos/cli-sub-agent \
  -p cli-sub-agent --all-features --locked
```

### From Source

```bash
git clone https://github.com/RyderFreeman4Logos/cli-sub-agent
cd cli-sub-agent
cargo install --all-features --path crates/cli-sub-agent
```

## Quick Start

### 1. Initialize Project

```bash
cd your-project
csa init
# Creates .csa/config.toml with detected tools
```

### 2. Run Tasks

```bash
# Analysis (read-only, uses gemini-cli)
csa run --tool gemini-cli "Analyze the authentication flow"

# Implementation (uses opencode/codex/claude-code)
csa run --tool opencode --session my-task "Fix the login bug"

# Resume an existing session
csa run --tool opencode --session 01JK... "Continue the refactor"

# Override model
csa run --tool opencode --model "provider/model-name" "Implement feature X"

# Ephemeral session (no project state, auto-cleanup)
csa run --tool gemini-cli --ephemeral "What is the CAP theorem?"
```

### 3. Manage Sessions

```bash
csa session list                          # List all sessions
csa session list --tree                   # Show parent-child tree
csa session list --tool opencode          # Filter by tool
csa session compress --session 01JK...    # Compress context window
csa session delete --session 01JK...      # Delete a session
```

### 4. Housekeeping

```bash
csa gc                # Garbage-collect orphaned sessions
csa config show       # Display current config
csa config validate   # Validate config file
```

## Supported Tools

| Tool | Yolo Flag | Context Compress | Edit Existing Files |
|------|-----------|-----------------|---------------------|
| gemini-cli | `--sandbox=false` | `/compress` | Restricted by default* |
| opencode | — | `/compact` | Yes |
| codex | `--full-auto` | `/compact` | Yes |
| claude-code | `--dangerously-skip-permissions` | `/compact` | Yes |

\* gemini-cli defaults to read-only for existing files to prevent accidental code/comment deletion. Override in `.csa/config.toml`.

## Architecture

CSA is a **Recursive Agent Container** built as a Rust workspace with 6 crates:

```
crates/
  cli-sub-agent/   # Binary crate — CLI entry point (clap)
  csa-config/      # Project configuration (.csa/config.toml)
  csa-core/        # Shared types, errors, validation
  csa-executor/    # Tool execution and model spec parsing
  csa-resource/    # Memory monitoring, usage stats, resource guard
  csa-session/     # Session CRUD, genealogy, locking
```

Key concepts:

| Concept | Description |
|---------|-------------|
| **Meta-Session** | Persistent workspace in `~/.local/state/csa/`, identified by ULID |
| **Genealogy** | Sessions track parent-child relationships, forming execution trees |
| **Resource Guard** | Pre-flight memory check using P95 historical estimates |
| **Tool Isolation** | File-level `flock` locks prevent concurrent tool conflicts |
| **Model Spec** | Unified `tool/provider/model/thinking_budget` addressing |

See [docs/architecture.md](docs/architecture.md) for the full design.

## Configuration

Project config lives at `.csa/config.toml`:

```toml
[project]
name = "my-project"
max_recursion_depth = 5

[tools.gemini-cli]
enabled = true
[tools.gemini-cli.restrictions]
allow_edit_existing_files = false   # Safe default

[tools.opencode]
enabled = true

[tiers.tier-1-quick]
description = "Quick lookups"
models = ["gemini-cli/google/gemini-2.5-flash/low"]

[tiers.tier-2-standard]
description = "Standard development"
models = ["opencode/anthropic/claude-sonnet-4-5/medium"]

[resources]
min_free_memory_mb = 512
```

### Tier-Based Auto-Selection

When `--tool` is omitted, CSA uses the `tier_mapping.default` entry from config to select a tool automatically:

```toml
[tier_mapping]
default = "tier-2-standard"       # Used when --tool is omitted
analysis = "tier-1-quick"         # For future keyword-based selection
```

The `default` tier resolves to the first model in the tier's `models` list, which determines both the tool and the model. To override, use `--tool` explicitly or `--model-spec tool/provider/model/thinking`.

See [docs/configuration.md](docs/configuration.md) for the full reference.

## Advanced Usage

### Recursive Agent Spawning

Any tool running inside CSA can call `csa` again to spawn sub-agents:

```bash
# Inside an agent session:
csa run --tool opencode --parent $CSA_SESSION_ID \
  "Research PostgreSQL extensions"
```

Recursion depth is tracked via `CSA_DEPTH` and limited (default: 5).

### Parallel Execution

```bash
# Safe: parallel reads
csa run --tool gemini-cli --session research-1 "Research topic A" &
csa run --tool gemini-cli --session research-2 "Research topic B" &
wait

# Then: serial writes
csa run --tool opencode --session impl "Implement based on research"
```

**Rules**:
- Parallel reads (analysis, search): **Safe**
- Parallel writes to isolated directories: Proceed with caution
- Parallel writes to shared files: **Forbidden**

See [docs/recursion.md](docs/recursion.md) for patterns.

### Environment Variables

CSA injects these into child processes:

| Variable | Description |
|----------|-------------|
| `CSA_SESSION_ID` | Current session ULID |
| `CSA_DEPTH` | Current recursion depth (0 = root) |
| `CSA_PROJECT_ROOT` | Absolute path to project root |
| `CSA_PARENT_SESSION` | Parent session ULID (if sub-agent) |

## Development

```bash
just fmt          # Format code
just clippy       # Lint (strict: -D warnings)
just test         # Run tests (cargo nextest)
just test-e2e     # E2E tests only
just pre-commit   # Full check suite
```

## License

MIT OR Apache-2.0
