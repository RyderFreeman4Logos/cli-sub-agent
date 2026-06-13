# Command Reference

Complete CLI reference for `csa`. All commands support `--format json` for
machine-readable output.

## `csa run` -- Execute a task

Run a prompt against an AI tool with session management and resource checks.

```bash
csa run --sa-mode false [OPTIONS] [PROMPT]
```

| Flag | Description |
|------|-------------|
| `--sa-mode <BOOL>` | Root callers must pass `true` or `false`; internal recursive calls default to `false` |
| `--tool <TOOL>` | Tool selection or preference: `auto` (default), `any-available`, or specific name. With `--tier`, this is a soft try-first preference |
| `--tier <NAME>` | Canonical selector when `[tiers]` is configured; accepts tier names, aliases, or unambiguous prefixes |
| `--auto-route <INTENT>` | Resolve routing intent through `[tier_mapping]` or a tier selector while keeping tool choice automatic |
| `--hint-difficulty <LABEL>` | Resolve a difficulty label through `[tier_mapping]` when no explicit `--tier` or permitted direct model bypass is set |
| `--skill <NAME>` | Run a named skill as a sub-agent |
| `--session <ID>` | Resume existing session (ULID or prefix) |
| `--last` | Resume the most recent session |
| `--description <TEXT>` | Description for a new session |
| `--ephemeral` | Ephemeral session (no project files, auto-cleanup) |
| `--model-spec <SPEC>` | Exact model spec: `tool/provider/model/thinking`. With configured tiers, rejected unless the global tier-policy escape hatch is enabled or an inherited trusted subtree pin is active |
| `--model <MODEL>` | Override tool default model |
| `--thinking <LEVEL>` | Thinking budget: `low`, `medium`, `high`, `xhigh` |
| `--force` | Emergency direct-routing override. With configured tiers, rejected unless the global tier-policy escape hatch is enabled |
| `--force-ignore-tier-setting` / `--force-tier` | Emergency tier bypass for direct tool/model routing. Invalid with `--tier`; rejected under configured tiers unless the global tier-policy escape hatch is enabled or CSA is continuing the same inherited subtree pin |
| `--no-failover` | Disable automatic 429 failover |
| `--wait` | Block-wait for a free slot instead of failing |
| `--idle-timeout <SECS>` | Kill when no output for N seconds |
| `--no-idle-timeout` | Disable idle-timeout killing |
| `--stream-stdout` | Force stdout streaming to stderr |
| `--no-stream-stdout` | Suppress real-time streaming |
| `--cd <DIR>` | Working directory |

If `PROMPT` is omitted, reads from stdin.

When `[tiers]` is non-empty, `--tier <name>` is the canonical way to pick
quality/cost/speed. `--tool`, `[review].tool`, and `[debate].tool` only
reorder the selected tier so preferred tools are tried in the order listed,
then remaining tier models are tried in tier order; they no longer hard-filter
the tier. Exact model and force-bypass flags are reserved
for emergency use and require `[tier_policy].allow_force_bypass = true` in the
global config, not project `.csa/config.toml`, unless CSA is continuing an
already-trusted inherited subtree pin.

Inside a model-pinned CSA subtree, nested workers should invoke
`csa run --skill ...`, `csa review`, or `csa debate` without repeating
`--model-spec` or `--force-ignore-tier-setting`. CSA passes the already
authorized exact pin through `CSA_MODEL_SPEC`; repeating the same inherited
spec remains accepted for older prompts, but changing the spec is treated as a
new bypass attempt.

**Examples:**

```bash
csa run --sa-mode false "fix the login bug"
csa run --sa-mode false --tier tier-2-standard "refactor error handling"
csa run --sa-mode false --tier tier-2-standard --tool codex "try codex first, then fall back through the tier"
csa run --sa-mode false --tool claude --hint-difficulty quick_question "answer briefly"
csa run --sa-mode false --auto-route analysis "trace the auth flow"
csa run --sa-mode false --last "continue where I left off"
echo "analyze this" | csa run --sa-mode false --tier tier-1-quick --tool gemini-cli
```

## `csa review` -- Code review

Review code changes using a heterogeneous AI model.

```bash
csa review --sa-mode false [OPTIONS]
```

| Flag | Description |
|------|-------------|
| `--sa-mode <BOOL>` | Root callers must pass `true` or `false`; internal recursive calls default to `false` |
| `--diff` | Review uncommitted changes (`git diff HEAD`) |
| `--range <RANGE>` | Review a commit range (e.g., `main...HEAD`) |
| `--commit <SHA>` | Review a specific commit |
| `--files <PATHSPEC>` | Review specific files |
| `--branch <BRANCH>` | Compare against branch (default: main) |
| `--tool <TOOL>` | Tool preference. With `--tier`, try this tool first and fall back through the rest of the tier |
| `--tier <NAME>` | Canonical selector when `[tiers]` is configured |
| `--hint-difficulty <LABEL>` | Resolve a difficulty label through `[tier_mapping]` when no explicit `--tier` or permitted direct model bypass is set |
| `--model <MODEL>` | Override model |
| `--force-ignore-tier-setting` / `--force-tier` | Emergency tier bypass; rejected under configured tiers unless the global tier-policy escape hatch is enabled or CSA is continuing the same inherited subtree pin |
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
csa review --sa-mode false --diff
csa review --sa-mode false --tier tier-2-standard --tool claude-code --diff
csa review --sa-mode false --range main...HEAD
csa review --sa-mode false --diff --reviewers 3 --consensus majority
csa review --sa-mode false --diff --fix --security-mode on
```

## `csa debate` -- Adversarial debate

Run a multi-round debate between heterogeneous AI tools.

```bash
csa debate --sa-mode false [OPTIONS] [QUESTION]
```

| Flag | Description |
|------|-------------|
| `--sa-mode <BOOL>` | Root callers must pass `true` or `false`; internal recursive calls default to `false` |
| `--tool <TOOL>` | Tool preference. With `--tier`, try this tool first and fall back through the rest of the tier |
| `--tier <NAME>` | Canonical selector when `[tiers]` is configured |
| `--hint-difficulty <LABEL>` | Resolve a difficulty label through `[tier_mapping]` when no explicit `--tier` or permitted direct model bypass is set |
| `--session <ID>` | Resume existing debate session |
| `--model <MODEL>` | Override model |
| `--thinking <LEVEL>` | Thinking budget |
| `--force-ignore-tier-setting` / `--force-tier` | Emergency tier bypass; rejected under configured tiers unless the global tier-policy escape hatch is enabled or CSA is continuing the same inherited subtree pin |
| `--rounds <N>` | Number of debate rounds (default: 3) |
| `--timeout <SECS>` | Absolute wall-clock timeout |
| `--idle-timeout <SECS>` | Kill on output silence |

**Examples:**

```bash
csa debate --sa-mode false "Should we use anyhow or thiserror?"
csa debate --sa-mode false --tier tier-3-complex --tool claude-code "Pick the storage boundary"
csa debate --sa-mode false --session 01JK "reconsider with performance data"
csa debate --sa-mode false --rounds 5 "Redis vs Memcached for session storage"
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

Show the last execution result. If the supplied ID is a resume wrapper returned
by `csa run --session`, the command follows the wrapper to the worker result.

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

List artifacts in a session's output directory, including resumed-turn manager
reports under `turns/turn-000001/result.toml`, `turns/turn-000002/result.toml`,
and later turn directories.

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
csa plan run --sa-mode false <FILE> [--var KEY=VALUE...] [--tool <TOOL>] [--dry-run] [--cd <DIR>]
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
| `csa batch --sa-mode false <FILE> [--dry-run]` | Execute tasks from a batch TOML file |
| `csa setup claude-code` | Setup MCP integration for Claude Code |
| `csa setup codex` | Setup MCP integration for Codex |
| `csa setup opencode` | Setup MCP integration for OpenCode |
| `csa migrate [--dry-run] [--status]` | Run pending config/state migrations |
| `csa self-update [--check]` | Update CSA to the latest release |
| `csa mcp-server` | Run as MCP server (JSON-RPC over stdio) |
