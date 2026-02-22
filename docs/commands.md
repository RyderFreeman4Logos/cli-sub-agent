# Command Reference

Complete CLI reference for `csa`. All commands support `--format json` for
machine-readable output.

## `csa run` -- Execute a task

Run a prompt against an AI tool with session management and resource checks.

```bash
csa run [OPTIONS] [PROMPT]
```

| Flag | Description |
|------|-------------|
| `--tool <TOOL>` | Tool selection: `auto` (default), `any-available`, or specific name |
| `--skill <NAME>` | Run a named skill as a sub-agent |
| `--session <ID>` | Resume existing session (ULID or prefix) |
| `--last` | Resume the most recent session |
| `--description <TEXT>` | Description for a new session |
| `--ephemeral` | Ephemeral session (no project files, auto-cleanup) |
| `--model-spec <SPEC>` | Full model spec: `tool/provider/model/thinking` |
| `--model <MODEL>` | Override tool default model |
| `--thinking <LEVEL>` | Thinking budget: `low`, `medium`, `high`, `xhigh` |
| `--force` | Bypass tier whitelist enforcement |
| `--no-failover` | Disable automatic 429 failover |
| `--wait` | Block-wait for a free slot instead of failing |
| `--idle-timeout <SECS>` | Kill when no output for N seconds |
| `--no-idle-timeout` | Disable idle-timeout killing |
| `--stream-stdout` | Force stdout streaming to stderr |
| `--no-stream-stdout` | Suppress real-time streaming |
| `--cd <DIR>` | Working directory |

If `PROMPT` is omitted, reads from stdin.

**Examples:**

```bash
csa run "fix the login bug"
csa run --tool codex --thinking high "refactor error handling"
csa run --model-spec "codex/openai/gpt-5.3-codex/xhigh" "complex task"
csa run --last "continue where I left off"
echo "analyze this" | csa run --tool gemini-cli
```

## `csa review` -- Code review

Review code changes using a heterogeneous AI model.

```bash
csa review [OPTIONS]
```

| Flag | Description |
|------|-------------|
| `--diff` | Review uncommitted changes (`git diff HEAD`) |
| `--range <RANGE>` | Review a commit range (e.g., `main...HEAD`) |
| `--commit <SHA>` | Review a specific commit |
| `--files <PATHSPEC>` | Review specific files |
| `--branch <BRANCH>` | Compare against branch (default: main) |
| `--tool <TOOL>` | Override tool selection |
| `--model <MODEL>` | Override model |
| `--fix` | Review-and-fix mode (apply fixes directly) |
| `--security-mode <MODE>` | `auto`, `on`, or `off` |
| `--reviewers <N>` | Number of parallel reviewers (default: 1) |
| `--consensus <STRATEGY>` | `majority`, `weighted`, or `unanimous` |
| `--context <FILE>` | Path to context file (e.g., TODO plan) |
| `--timeout <SECS>` | Absolute wall-clock timeout |
| `--idle-timeout <SECS>` | Kill on output silence |
| `--allow-fallback` | Warn instead of error when pattern missing |
| `--session <ID>` | Resume existing review session |

**Examples:**

```bash
csa review --diff
csa review --range main...HEAD
csa review --diff --reviewers 3 --consensus majority
csa review --diff --fix --security-mode on
```

## `csa debate` -- Adversarial debate

Run a multi-round debate between heterogeneous AI tools.

```bash
csa debate [OPTIONS] [QUESTION]
```

| Flag | Description |
|------|-------------|
| `--tool <TOOL>` | Override tool selection |
| `--session <ID>` | Resume existing debate session |
| `--model <MODEL>` | Override model |
| `--thinking <LEVEL>` | Thinking budget |
| `--rounds <N>` | Number of debate rounds (default: 3) |
| `--timeout <SECS>` | Absolute wall-clock timeout |
| `--idle-timeout <SECS>` | Kill on output silence |

**Examples:**

```bash
csa debate "Should we use anyhow or thiserror?"
csa debate --session 01JK "reconsider with performance data"
csa debate --rounds 5 "Redis vs Memcached for session storage"
```

## `csa session` -- Session management

### `csa session list`

```bash
csa session list [--tree] [--tool <TOOLS>] [--branch <BRANCH>] [--cd <DIR>]
```

### `csa session compress`

Send tool-specific compression command (`/compress` or `/compact`).

```bash
csa session compress --session <ID> [--cd <DIR>]
```

### `csa session delete`

```bash
csa session delete --session <ID> [--cd <DIR>]
```

### `csa session clean`

Remove sessions not accessed within N days.

```bash
csa session clean --days <N> [--dry-run] [--tool <TOOLS>] [--cd <DIR>]
```

### `csa session result`

Show the last execution result.

```bash
csa session result --session <ID> [--json] [--cd <DIR>]
```

### `csa session logs`

```bash
csa session logs --session <ID> [--tail <N>] [--cd <DIR>]
```

### `csa session is-alive`

Check whether a session is still running via filesystem liveness signals.

```bash
csa session is-alive --session <ID> [--cd <DIR>]
```

### `csa session artifacts`

List artifacts in a session's output directory.

```bash
csa session artifacts --session <ID> [--cd <DIR>]
```

### `csa session log`

Show git history for a session.

```bash
csa session log --session <ID> [--cd <DIR>]
```

### `csa session checkpoint`

Write a checkpoint note (git notes) for audit trail.

```bash
csa session checkpoint --session <ID> [--cd <DIR>]
```

### `csa session checkpoints`

List all checkpoint notes.

```bash
csa session checkpoints [--cd <DIR>]
```

## `csa config` -- Configuration management

### `csa config show`

Display effective merged configuration.

```bash
csa config show [--cd <DIR>]
```

### `csa config edit`

Open project config in `$EDITOR`.

```bash
csa config edit [--cd <DIR>]
```

### `csa config validate`

Validate configuration file syntax and references.

```bash
csa config validate [--cd <DIR>]
```

### `csa config get`

Query a single config value by dotted key path.

```bash
csa config get <KEY> [--default <VALUE>] [--project] [--global] [--cd <DIR>]
```

**Examples:**

```bash
csa config get review.tool
csa config get tools.codex.enabled --default true
csa config get review.tool --global
```

## `csa todo` -- Plan management

### `csa todo create`

```bash
csa todo create <NAME>
```

### `csa todo show`

```bash
csa todo show -t <TIMESTAMP>
```

### `csa todo diff`

```bash
csa todo diff -t <TIMESTAMP> --from <VER> --to <VER>
```

### `csa todo dag`

```bash
csa todo dag --format mermaid
```

### `csa todo list`

```bash
csa todo list [--status <STATUS>]
```

### `csa todo status`

```bash
csa todo status <TIMESTAMP> <STATUS>
```

## `csa plan` -- Workflow execution

Execute compiled weave workflow files.

```bash
csa plan run <FILE> [--var KEY=VALUE...] [--tool <TOOL>] [--dry-run] [--cd <DIR>]
```

## `csa mcp-hub` -- MCP Hub daemon

### `csa mcp-hub serve`

```bash
csa mcp-hub serve [--background] [--foreground] [--socket <PATH>] [--systemd-activation]
```

### `csa mcp-hub status`

```bash
csa mcp-hub status [--socket <PATH>]
```

### `csa mcp-hub stop`

```bash
csa mcp-hub stop [--socket <PATH>]
```

### `csa mcp-hub gen-skill`

Regenerate the mcp-hub routing-guide skill from live `tools/list`.

```bash
csa mcp-hub gen-skill [--socket <PATH>]
```

## `csa skill` -- Skill management

### `csa skill install`

```bash
csa skill install <SOURCE> [--target <TOOL>]
```

### `csa skill list`

```bash
csa skill list
```

## `csa audit` -- Codebase audit tracking

### `csa audit init`

```bash
csa audit init [--root <PATH>] [--ignore <PATTERN>...] [--mirror-dir <DIR>]
```

### `csa audit status`

```bash
csa audit status [--format text|json] [--filter <STATUS>] [--order topo|depth|alpha]
```

### `csa audit update`

```bash
csa audit update <FILES...> [--status <STATUS>] [--auditor <NAME>]
```

### `csa audit approve`

```bash
csa audit approve <FILES...> [--approved-by <NAME>]
```

### `csa audit reset` / `csa audit sync`

```bash
csa audit reset <FILES...>
csa audit sync
```

## Operations Commands

| Command | Description |
|---------|-------------|
| `csa init [--full] [--template]` | Initialize project configuration |
| `csa doctor` | Check environment and tool availability |
| `csa gc [--dry-run] [--max-age-days N] [--global]` | Garbage collect expired sessions and locks |
| `csa tiers list` | List configured tiers with model specs |
| `csa batch <FILE> [--dry-run]` | Execute tasks from a batch TOML file |
| `csa setup claude-code` | Setup MCP integration for Claude Code |
| `csa setup codex` | Setup MCP integration for Codex |
| `csa setup opencode` | Setup MCP integration for OpenCode |
| `csa migrate [--dry-run] [--status]` | Run pending config/state migrations |
| `csa self-update [--check]` | Update CSA to the latest release |
| `csa mcp-server` | Run as MCP server (JSON-RPC over stdio) |
