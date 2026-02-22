# CSA (CLI Sub-Agent)

> **Recursive Agent Container** -- Composable Unix processes for orchestrating LLM CLI tools

[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange)](https://www.rust-lang.org/)

CSA is a **headless IDE runtime for AI agents**. It provides a unified CLI that
orchestrates multiple AI coding tools (claude-code, codex, gemini-cli, opencode)
as composable Unix processes -- enabling recursive sub-agent spawning,
model-heterogeneous code review, and resource-aware scheduling.

## Why CSA?

Built-in sub-agents and agent-teams are **homogeneous** -- they only use one
model family. Reviews suffer from same-model blind spots. CSA enforces
**heterogeneous execution**: if the parent is Claude, the reviewer is
automatically Codex or Gemini, and vice versa. No silent fallback.

## Key Features

| Feature | Description |
|---------|-------------|
| **Recursive Agents** | Any agent can spawn sub-agents via `csa run`, up to configurable depth (default: 5) |
| **Heterogeneous Review** | `csa review` and `csa debate` auto-select a different model family than the caller |
| **ACP Transport** | Precise context injection via Agent Communication Protocol, replacing 60K+ token auto-load |
| **MCP Hub** | Shared MCP server fan-out daemon with FIFO queuing and stateful pooling |
| **Resource Sandbox** | Three-layer defense: cgroup v2, setrlimit, RSS monitor -- with P95 memory estimation |
| **Session Persistence** | ULID-based sessions with genealogy, checkpoints (git-notes), and prefix matching |
| **Skill-as-Agent** | Skills package complete agent definitions (prompt + tools + protocol) |
| **Weave Compiler** | skill-lang patterns compile to deterministic workflow plans (`workflow.toml`) |
| **Consensus Engine** | Multi-reviewer with majority / unanimous / weighted strategies |
| **Config-Driven** | Tool selection and thinking budget from tiered config; CLI flags are overrides |

## Quick Start

```bash
# Install via mise (recommended -- no Rust toolchain needed)
mise use -g ubi:RyderFreeman4Logos/cli-sub-agent[exe=csa]
mise use -g ubi:RyderFreeman4Logos/cli-sub-agent[exe=weave]

# Or install via script
curl -fsSL https://raw.githubusercontent.com/RyderFreeman4Logos/cli-sub-agent/main/install.sh | sh

# Initialize a project
cd my-project && csa init && csa doctor

# Run a task
csa run "implement user auth module"

# Code review (auto-selects heterogeneous model)
csa review --diff

# Adversarial debate
csa debate "Should we use Redis or Memcached for caching?"
```

See [Getting Started](docs/getting-started.md) for full installation and setup instructions.

## Architecture

```
Main Agent (depth=0, claude-code)
  |-- Reviewer (depth=1, codex)         # heterogeneous review
  |   +-- Analyzer (depth=2, gemini)    # deep analysis
  +-- Debater (depth=1, codex)          # adversarial debate
      +-- Adversary (depth=2, claude)   # counter-argument
```

CSA is organized as 14 workspace crates:

| Crate | Purpose |
|-------|---------|
| `cli-sub-agent` | Main CLI binary (`csa`) |
| `csa-core` | Core types (ToolName, ULID, OutputFormat) |
| `csa-acp` | ACP transport (AcpConnection, AcpSession) |
| `csa-session` | Session CRUD, genealogy, transcripts |
| `csa-executor` | Tool executor (closed enum, Transport trait) |
| `csa-process` | Process spawning, signals, sandbox integration |
| `csa-config` | Global + project config merging, migrations |
| `csa-resource` | Memory estimation, cgroup/rlimit sandbox |
| `csa-scheduler` | Tier rotation, 429 failover, concurrency slots |
| `csa-mcp-hub` | MCP server fan-out daemon |
| `csa-hooks` | Lifecycle hooks and prompt guards |
| `csa-todo` | Git-tracked TODO/plan management |
| `csa-lock` | flock-based session and slot locking |
| `weave` | skill-lang compiler (`weave` binary) |

See [Architecture](docs/architecture.md) for design principles and dependency graph.

## Supported Tools

| Tool | Provider | Transport | Context |
|------|----------|-----------|---------|
| **claude-code** | Anthropic | ACP | 200K |
| **codex** | OpenAI | ACP | 200K |
| **gemini-cli** | Google | Legacy CLI | 2M |
| **opencode** | OpenRouter | Legacy CLI | 200K |

## Documentation

| Chapter | Description |
|---------|-------------|
| [Getting Started](docs/getting-started.md) | Installation, first run, project setup |
| [Architecture](docs/architecture.md) | Crate structure, design principles, data flow |
| [Commands](docs/commands.md) | Complete CLI reference with flags and examples |
| [Configuration](docs/configuration.md) | Global/project config, tiers, aliases |
| [Sessions](docs/sessions.md) | Session lifecycle, genealogy, checkpoints |
| [ACP Transport](docs/acp-transport.md) | Agent Communication Protocol, context injection |
| [MCP Hub](docs/mcp-hub.md) | Shared MCP daemon, proxy injection, FIFO queue |
| [Resource Control](docs/resource-control.md) | Sandbox, cgroup, rlimits, P95 estimation |
| [Skills & Patterns](docs/skills-patterns.md) | Skill system, weave compiler, workflow.toml |
| [Hooks](docs/hooks.md) | Lifecycle hooks and prompt guards |
| [Debate & Review](docs/debate-review.md) | Heterogeneous review, consensus engine |
| [MCP Hub on macOS](docs/mcp-hub-launchd.md) | launchd integration guide |

## Development

```bash
# Requirements: Rust 1.85+, just
just clippy       # Build + lint
just test         # Run tests
just fmt          # Format
just pre-commit   # Full pre-commit check
```

## License

Apache-2.0
