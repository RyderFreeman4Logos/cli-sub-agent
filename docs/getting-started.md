# Getting Started

This guide covers installation, initial setup, and your first CSA commands.

## Prerequisites

- At least one supported AI tool installed:
  - [claude-code](https://docs.anthropic.com/en/docs/claude-code) (Anthropic)
  - [codex](https://github.com/openai/codex) (OpenAI)
  - [gemini-cli](https://github.com/google-gemini/gemini-cli) (Google)
  - [opencode](https://github.com/sst/opencode) (OpenRouter)
- Git 2.30+
- Optional: [mise](https://mise.jdx.dev/) for binary version management
- Optional: [gh](https://cli.github.com/) for PR workflows

## Installation

### Option 1: mise (recommended)

[mise](https://mise.jdx.dev/) downloads prebuilt binaries from GitHub Releases
via the [ubi](https://github.com/houseabsolute/ubi) backend. No Rust toolchain
required.

```bash
# Install mise if needed
curl https://mise.run | sh

# Install csa and weave
mise use -g ubi:RyderFreeman4Logos/cli-sub-agent[exe=csa]
mise use -g ubi:RyderFreeman4Logos/cli-sub-agent[exe=weave]

# Verify
csa --version
weave --help

# Upgrade later
mise upgrade
```

### Option 2: Install script

```bash
# Prebuilt binary
curl -fsSL https://raw.githubusercontent.com/RyderFreeman4Logos/cli-sub-agent/main/install.sh | sh

# Or build from source via the install script
curl -fsSL https://raw.githubusercontent.com/RyderFreeman4Logos/cli-sub-agent/main/install.sh | sh -s -- --from-source
```

### Option 3: Build from source

Requires Rust 1.85+ (`rustc --version`).

```bash
git clone https://github.com/RyderFreeman4Logos/cli-sub-agent.git
cd cli-sub-agent
cargo install --path crates/cli-sub-agent   # Installs `csa`
cargo install --path crates/weave           # Installs `weave`
```

## Setup via AI Agent

If you use an AI coding agent (Claude Code, Codex, etc.), paste this prompt
into a new session to run the full setup automatically:

```
Read https://raw.githubusercontent.com/RyderFreeman4Logos/cli-sub-agent/main/skill.md and follow the steps to configure CSA and programming workflow patterns for this project.
```

The agent will install CSA and Weave, initialize your project, and
interactively select workflow patterns (commit, review, security audit,
planning) -- all guided by [`skill.md`](../skill.md).

## Manual Project Setup

### 1. Initialize

```bash
cd my-project
csa init
```

This creates `.csa/config.toml` with project metadata. Use `csa init --full`
to auto-detect available tools and generate tier configuration, or
`csa init --template` for a fully-commented reference config.

### 2. Check tool availability

```bash
csa doctor
```

Reports which AI tools are installed and reachable.

### 3. Configure (optional)

Edit `.csa/config.toml` to customize tiers, enable/disable tools, and set
resource limits. See [Configuration](configuration.md) for the full schema.

For global settings (API keys, concurrency limits), edit
`~/.config/cli-sub-agent/config.toml`.

## First Commands

### Run a task

```bash
# Auto-select tool from tier config
csa run "analyze the authentication flow"

# Specify a tool
csa run --tool codex "implement user auth module"

# Auto-select tool (explicit)
csa run --tool auto "fix login page bug"
```

### Resume a session

```bash
# Resume the most recent session
csa run --last "continue the implementation"

# Resume by ULID prefix
csa run --session 01JK "continue the refactor"
```

### Code review

```bash
# Review uncommitted changes (auto-selects heterogeneous model)
csa review --diff

# Review a commit range
csa review --range main...HEAD

# Multi-reviewer consensus
csa review --diff --reviewers 3 --consensus majority
```

### Adversarial debate

```bash
csa debate "Should we use anyhow or thiserror for error handling?"
```

### Session management

```bash
csa session list              # List all sessions
csa session list --tree       # Tree view with genealogy
csa session result -s 01JK    # View execution result
```

## Global Configuration

Create `~/.config/cli-sub-agent/config.toml` for settings shared across
all projects:

```toml
[defaults]
max_concurrent = 3
tool = "claude-code"        # Fallback for --tool auto

[review]
tool = "auto"               # Enforce heterogeneous review

[tools.codex]
max_concurrent = 5
[tools.codex.env]
OPENAI_API_KEY = "sk-..."
```

## Next Steps

- [Commands](commands.md) -- full CLI reference
- [Configuration](configuration.md) -- tiers, aliases, resource limits
- [Sessions](sessions.md) -- session lifecycle and genealogy
- [Architecture](architecture.md) -- crate structure and design principles
