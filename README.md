# CSA (CLI Sub-Agent)

> **Recursive Agent Container**: A standardized, composable Unix process management system for LLM CLI tools

CSA provides a unified command-line interface for multiple AI coding tools (claude-code, codex, gemini-cli, opencode), enabling Agents to safely spawn sub-Agents recursively and perform adversarial code review through model-heterogeneous strategies.

## What is CSA?

CSA takes the same idea as Cursor and Zed — structured, protocol-driven communication with AI coding tools — and exposes it as composable Unix processes. Think of it as a **headless IDE runtime for AI agents**: no GUI, multi-provider, focused on orchestration rather than editing.

**CSA's goal is to fully replace the built-in agent mechanisms of individual AI coding tools**, providing a unified, tool-agnostic orchestration layer:

| Built-in feature | CSA replacement | Key advantage |
|---|---|---|
| Claude Code sub-agents (Task tool) | `csa run` recursive agent tree | Multi-provider + heterogeneous models |
| Claude Code plan mode | `csa mktd` + mktsk-codex pattern | Git-tracked TODO with DAG visualization |
| Claude Code agent-teams | CSA recursive agent tree + Skills | Nested teams, multi-provider, ACP context control |
| Codex review / `/review` | `csa review --diff` | Enforced heterogeneous model (never self-review) |

### Why replace built-in mechanisms?

Built-in sub-agents and agent-teams are **homogeneous** — they only use the same model family (e.g., Claude Code's agent-teams spawn only Claude instances). This means reviews and debates suffer from same-model blind spots. CSA enforces **heterogeneous execution**: if the parent agent is Claude, the review agent is automatically switched to Codex or Gemini, and vice versa.

### Compared to GUI IDEs

| | Cursor | Zed | CSA |
|---|---|---|---|
| **Interface** | GUI IDE (VS Code fork) | GUI editor | CLI process container |
| **AI providers** | Multiple (Anthropic, OpenAI, Google, etc.) | Multiple (Anthropic, OpenAI, Google, etc.) | Multiple via ACP |
| **Multi-provider** | One active at a time | One active at a time | Simultaneous (e.g., Claude writes code while Codex reviews it) |
| **Primary focus** | Code completion + inline chat | Code completion + inline chat | Agent recursive orchestration + heterogeneous review |
| **Composability** | GUI-bound | GUI-bound | Unix composable (pipes, scripts, sub-processes) |

### Compared to built-in agent-teams

| | Claude Code agent-teams | CSA |
|---|---|---|
| **Model diversity** | Homogeneous (Claude only) | Heterogeneous (Claude + Codex + Gemini + OpenRouter) |
| **Nesting** | No nested teams | Recursive to depth 5 (configurable) |
| **Context control** | Full project context auto-loaded | ACP precise injection (only task-relevant skills) |
| **Workflow definition** | Natural language prompts | skill-lang patterns compiled to deterministic plans |
| **Persistence** | Session-bound, no resumption | ULID sessions with git-notes checkpoints |
| **Resource control** | None | P95 memory estimation + global concurrency slots |

## Core Features

- **Recursive Agent Container** -- Any Agent running inside CSA can invoke `csa` again to spawn sub-Agents, with recursion depth limited by the `CSA_DEPTH` environment variable (default: 5)
- **Model-Heterogeneous Strategy** -- Review/Debate always use different model families (for example, main Agent on Claude, review Agent auto-switched to Codex), eliminating self-review blind spots from single-model workflows
- **ACP Transport Layer** -- Uses ACP (Agent Communication Protocol) for precise context window control, replacing the 60K+ token full-load behavior of CLI non-interactive mode
- **Resource-Aware Scheduling** -- P95 memory estimation, global concurrency slots (`flock`), and automatic failover for 429/rate-limit scenarios
- **Git-Tracked TODO** -- Deep integration between planning and version control, with DAG visualization and multi-version traceability
- **Skill-as-Agent** -- 17 Skills package complete Agent definitions (prompt + tools + protocol), so the main Agent only needs to orchestrate
- **skill-lang and weave** -- Built-in skill-lang compiler; 11 workflow patterns are already compiled into deterministic execution plans

## Quick Start

### Installation

#### Quick install (prebuilt binaries)

```bash
curl -fsSL https://raw.githubusercontent.com/RyderFreeman4Logos/cli-sub-agent/main/install.sh | sh

# Or compile from source
curl -fsSL https://raw.githubusercontent.com/RyderFreeman4Logos/cli-sub-agent/main/install.sh | sh -s -- --from-source
```

#### Recommended: use mise (cross-platform tool version manager)

[mise](https://mise.jdx.dev/) can install `csa` and `weave` with one command and automatically manage version upgrades:

```bash
# Install mise (if not already installed)
curl https://mise.run | sh

# Install csa and weave
mise use -g ubi:RyderFreeman4Logos/cli-sub-agent[exe=csa]
mise use -g ubi:RyderFreeman4Logos/cli-sub-agent[exe=weave]

# Verify
csa --version
weave --help
```

> **Why mise?** Through the [ubi](https://github.com/houseabsolute/ubi) backend, mise downloads prebuilt binaries directly from GitHub Releases, with no local Rust toolchain required. Upgrade with a single `mise upgrade`.

#### Build from source

```bash
git clone https://github.com/RyderFreeman4Logos/cli-sub-agent.git
cd cli-sub-agent
cargo install --path crates/cli-sub-agent   # Install csa
cargo install --path crates/weave           # Install weave
```

### Setup via AI Agent (recommended)

If you use an AI coding agent (Claude Code, Codex, etc.), paste this prompt
into a new session to run the full setup automatically:

```
Read https://raw.githubusercontent.com/RyderFreeman4Logos/cli-sub-agent/main/skill.md and follow the steps to configure CSA and programming workflow patterns for this project.
```

The agent will install CSA, Weave, core skills, and interactively select
workflow patterns (commit/review, security audit, planning, etc.) — all
guided by [`skill.md`](skill.md).

### Initialize a project (manual)

```bash
cd my-project
csa init                    # Initialize .csa/ config directory
csa doctor                  # Check all tools availability
weave compile PATTERN.md    # Compile a skill-lang pattern
```

### Basic usage

```bash
# Run a task (specify tool)
csa run --tool codex "implement user auth module"

# Run a task (auto-select tool)
csa run --tool auto "fix login page bug"

# Resume last session
csa run --last "continue the implementation"

# Output streams to stderr by default for text output
# Use --no-stream-stdout to suppress
csa run --tool claude-code "refactor error handling"
```

### Code review (heterogeneous models)

```bash
# Review uncommitted changes (auto-selects heterogeneous model)
csa review --diff

# Review an entire PR
csa review --range main...HEAD

# Multi-reviewer consensus
csa review --diff --reviewers 3 --consensus majority
```

### Adversarial debate

```bash
# Technical design decisions
csa debate "Should we use anyhow or thiserror for error handling?"

# Continue debate (resume session by ULID prefix)
csa debate --session 01JK "re-evaluate considering performance impact"
```

## Architecture Overview

### Recursive Agent tree

CSA is built around a **fractal architecture**: each Agent is an independent Unix process that can recursively spawn sub-Agents:

```
Main Agent (depth=0, claude-code)
  |-- Sub-Agent-1 (depth=1, codex)        # review
  |   |-- Sub-Agent-1.1 (depth=2, gemini) # deep analysis
  |   +-- Sub-Agent-1.2 (depth=2, codex)  # fix implementation
  +-- Sub-Agent-2 (depth=1, codex)        # debate
      +-- Sub-Agent-2.1 (depth=2, claude) # adversary
```

Each Agent layer automatically inherits environment variables: `CSA_SESSION_ID`, `CSA_DEPTH`, `CSA_PROJECT_ROOT`, `CSA_TOOL`, `CSA_PARENT_TOOL`.

### Process tree detection

CSA automatically detects the parent tool via the `/proc` filesystem. In `--tool auto` mode, it selects a tool from a **different model family** than the parent for review/debate to guarantee heterogeneity. If no heterogeneous tool is available, CSA fails with an explicit error and never silently degrades to the same model.

### Crate architecture

```
workspace.members:
|-- csa-core          # Core types (ToolName, ULID, OutputFormat)
|-- csa-session       # Session management (create, load, state persistence)
|-- csa-lock          # Locking (session locks, slot locks)
|-- csa-executor      # Tool executor (enum dispatch, Transport trait)
|-- csa-process       # Process management (spawn, signals, process tree)
|-- csa-config        # Configuration (global + project-level merging)
|-- csa-resource      # Resource management (memory estimation, scheduling)
|-- csa-scheduler     # Scheduler (resource checks, concurrency control)
|-- csa-todo          # TODO system (git-tracked plan management)
|-- csa-hooks         # Hooks system (session.complete, etc.)
|-- csa-acp           # ACP transport layer (merged in PR #75)
+-- weave             # skill-lang compiler (parse, compile, execute)
```

## ACP Transport Layer

> ✅ **Epic 1 Complete**: All five phases (infrastructure → transport abstraction → pipeline integration → suppress_notify cleanup → testing) are implemented and merged (PR #75).

### Why ACP?

CSA previously launched tools through CLI non-interactive mode. Each launch in that mode auto-loaded CLAUDE.md + AGENTS.md + all skills + all MCP servers (60K+ tokens), significantly reducing available context for sub-Agents.

**ACP (Agent Communication Protocol)** uses `session/new` to control initialization context precisely, injecting only task-relevant skills/rules and loading progressively on demand. This saves tokens and, more importantly, protects scarce context window capacity.

### Transport routing

| Tool | Default Transport | ACP Command |
|------|---------------|----------|
| claude-code | ACP | `claude-code-acp` |
| codex | ACP | `codex-acp` |
| gemini-cli | Legacy | `gemini --experimental-acp` (not enabled by default) |
| opencode | Legacy | `opencode acp` |

The Transport trait abstracts both ACP and Legacy execution modes. `TransportFactory` routes automatically based on tool type and config. ACP fallback to Legacy is allowed only during connection initialization. During prompt execution, automatic fallback is forbidden.

### Context window control

```toml
# .skill.toml -- Control sub-agent context loading
[context]
no_load = ["CLAUDE.md", "AGENTS.md"]  # Skip default files
extra_load = ["./rules/security.md"]   # Load additional files
```

CSA’s MCP registry (`.csa/mcp.toml`) supports step-level MCP server injection, instead of loading every MCP server from the tool’s global configuration.

## Supported Tools

| Tool | Provider | Highlights | Session Resume | File Editing | Context |
|------|--------|------|---------|---------|--------|
| **claude-code** | Anthropic | Strong reasoning | ✅ | ✅ | 200K |
| **codex** | OpenAI | Lightweight and fast (Rust implementation) | ✅ | ✅ | 200K |
| **gemini-cli** | Google | Extremely large context | -- | -- | 2M |
| **opencode** | OpenRouter | Multi-model aggregation | ✅ | ✅ | 200K |

### Heterogeneous routing (Auto mode)

| Parent Tool | Review Tool | Reason |
|--------|------------|------|
| claude-code | codex or gemini-cli | Different model family |
| codex | claude-code or gemini-cli | Different model family |
| gemini-cli | claude-code or codex | Different model family |

### Tier system

| Tier | Use Case | Default Model |
|------|------|---------|
| tier-1-quick | Documentation, Q&A | codex/gGPT-5.3-Codex-Spark |
| tier-2-standard | Feature implementation, bug fixes | codex/claude-sonnet-4-5 |
| tier-3-complex | Architecture design, security audit | claude-code/claude-opus-4-6 |

## Configuration

### Configuration precedence

```
Global config (~/.config/cli-sub-agent/config.toml)
    | lowest priority
Project config ({PROJECT_ROOT}/.csa/config.toml)
    | higher priority
CLI arguments (--tool, --model, etc.)
    | highest priority
Final merged config
```

### Example global config

```toml
# ~/.config/cli-sub-agent/config.toml

[defaults]
max_concurrent = 3
tool = "claude-code"             # Final fallback for --tool auto

[review]
tool = "auto"                    # Enforce heterogeneous

[debate]
tool = "auto"                    # Enforce heterogeneous

[tools.codex]
max_concurrent = 5
[tools.codex.env]
OPENAI_API_KEY = "sk-..."

[tools.claude-code]
max_concurrent = 3

[todo]
show_command = "bat -l md --paging=always"
diff_command = "delta"
```

### Configuration commands

```bash
csa config show                  # Show effective config
csa config get review.tool       # Query a single key
csa config edit                  # Edit project config
csa config validate              # Validate config
```

### Hooks

Hook event timing and per-event template variables are documented in [`docs/hooks.md`](docs/hooks.md).

## Command Reference

### Core commands

```bash
# Run tasks
csa run --tool <tool> [--session <id>|--last] [--no-stream-stdout] "prompt"
csa run --model-spec codex/openai/gpt-5.3-codex/high "prompt"  # Full model spec

# Code review
csa review --diff                                # Review uncommitted changes
csa review --range main...HEAD                   # Review commit range
csa review --diff --reviewers 3 --consensus majority  # Multi-reviewer

# Adversarial debate
csa debate "technical question"
csa debate --session 01JK "continue debate"

# Session management
csa session list [--tree]                        # List sessions (tree view)
csa session compress --session <id>              # Compress session context
csa session result --session <id>                # View execution result
csa session checkpoint --session <id>            # Write audit checkpoint
csa session checkpoints                          # List all checkpoints
```

### Plan management

```bash
csa todo create "plan name"                       # Create a TODO
csa todo show -t <timestamp>                     # View details
csa todo diff -t <timestamp> --from 2 --to 1     # Compare versions
csa todo dag --format mermaid                    # DAG visualization
csa todo list --status implementing              # Filter by status
csa todo status <timestamp> done                 # Update status
```

### Operations commands

```bash
csa init                                         # Initialize project
csa doctor                                       # Diagnose tool availability
csa gc [--dry-run] [--global]                    # Clean up expired sessions
csa tiers list                                   # View tier definitions
csa skill install <source>                       # Install skills
csa self-update --check                          # Check for updates
```

## Session Management

CSA uses **ULID** session identifiers and supports prefix matching (similar to git hashes):

```bash
csa session list                   # List all sessions
csa session result -s 01JK         # Prefix matching
csa run --session 01JKABC "..."    # Resume a specific session
```

**Storage location**: `~/.local/state/csa/{project_path}/sessions/`

Sessions use flat physical storage with a logical tree structure. Parent-child relationships are maintained via the `parent_id` field in `state.toml`. Session state machine: `Active` → `Available` (after compression) → `Retired` (after GC).

## Security and Resource Controls

| Mechanism | Description |
|------|------|
| **Yolo Mode** | Automatically adds non-interactive approval flags to all sub-Agents |
| **Recursion depth limit** | `CSA_DEPTH` environment variable, default max depth is 5 |
| **Signal propagation** | Forwards SIGTERM/SIGINT to child process groups to prevent zombie processes |
| **`flock` file locks** | Session-level locks + global slot locks |
| **P95 memory estimation** | Checks system available memory against tool historical P95 before spawn |
| **Global concurrency slots** | Limits concurrency per tool (for example, codex max 5) |
| **StreamMode** | Streams output to stderr by default for text output; suppressed with `--no-stream-stdout` |
| **TokenBudget** | Tier-level token budgets (soft threshold 75%, hard threshold 100%) |

## Roadmap

### ✅ Completed: ACP Transport (Epic 1, PR #75)

The `csa-acp` crate and Transport trait abstraction are fully implemented and merged. All five phases are complete: Phase A (`csa-acp` crate), Phase B (Transport trait / LegacyTransport / AcpTransport / TransportFactory), Phase C (pipeline integration), Phase D (full suppress_notify cleanup), and Phase E (tests passing `just pre-commit`). MVP covers claude-code + codex.

### Near-term: deferred epics

| Epic | Scope |
|------|------|
| **Epic 2: Dynamic Tools** | Stringify `ToolName` enum and support custom tool registration |
| **Epic 3: Session Resume** | ACP `session/load`, historical replay deduplication |
| **Pre-Release: Security** | Secure validation for token-like values, hardened egress policy |

### ✅ Completed: skill-lang and weave compiler (PR #80 ~ #83, #89)

The weave compiler and skill-lang workflow engine are implemented:

- **skill-lang = Markdown with structured conventions**; the compiler is the LLM (`weave compile`), and the runtime is CSA
- Two-layer representation: `PATTERN.md` (natural language source) → `workflow.toml` (deterministic execution plan)
- Naming system: **skill** (atomic unit) → **pattern** (composed workflow) → **loom** (git repository)
- 11 workflow skills converted to skill-lang patterns and successfully compiled
- Syntax support: `## Step N`, `IF/ELSE/ENDIF`, `FOR/IN/ENDFOR`, `INCLUDE`, `${VAR}`, and Hint lines (`Tool:/Tier:/OnFail:`)

### In progress: weave ecosystem expansion

- No centralized registry; publish with git push
- Target users: skill developers building by conversation through openclaw

## Development

### Requirements

- Rust edition 2024 (`rustc` ≥ 1.85)
- `just` (command runner)
- At least one supported AI tool (recommended: claude-code + codex)
- Optional: `mise` (recommended for managing csa/weave binary versions)

### Development commands

```bash
just clippy                      # Build + lint
just test                        # Run tests
just fmt                         # Format
just pre-commit                  # Full pre-commit check (fmt + clippy + test)
cargo run -- <args>              # Run directly
```

### Coding conventions

- Error handling: `anyhow` (application layer) + `thiserror` (library layer)
- Async: `tokio` (`LocalSet` is used in the ACP layer for handling `!Send` futures)
- Tool abstraction: closed Enum (4 tool types), not Trait/Dynamic Dispatch
- Serialization: TOML for config/state, with `serde`
- Logging: `tracing`, isolated by session
- Commits: Conventional Commits, with scope aligned to crate names

### Project structure

```
cli-sub-agent/
|-- crates/                        # 13 Rust crates
|   |-- cli-sub-agent/             # Main CLI entry (binary: csa)
|   |-- csa-core/                  # Core types
|   |-- csa-session/               # Session management
|   |-- csa-executor/              # Tool executor (Transport trait)
|   |-- csa-acp/                   # ACP transport layer
|   |-- weave/                     # skill-lang compiler (binary: weave)
|   +-- ...
|-- skills/                        # 17 Agent Skills
|-- drafts/patterns/               # 11 skill-lang workflow patterns
|-- .csa/                          # Project-level config
|-- drafts/                        # Design docs (external symlink)
+-- Cargo.toml                     # Workspace config
```

## License

Apache-2.0

---

**Document version**: v1.4 | **Last updated**: 2026-02-18 | **Aligned PRs**: #57 ~ #151
