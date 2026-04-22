# ACP Transport

CSA uses the **Agent Communication Protocol (ACP)** for precise context
window control when communicating with `claude-code` and other ACP-backed
tools. `claude-code` defaults to ACP today, but you can opt into the native
CLI path with `[tools.claude-code].transport = "cli"`. Codex also defaults to
ACP today: current builds probe `codex-acp` by default, and
`[tools.codex].transport = "acp"` simply makes that explicit. Config
validation checks whether a transport value is legal for the tool; missing
runtime binaries are surfaced separately by `csa doctor`. For ACP sessions,
this replaces the CLI non-interactive mode which auto-loads 60K+ tokens of
project context.

## Why ACP?

In CLI non-interactive mode, each tool launch auto-loads CLAUDE.md, AGENTS.md,
all skills, and all MCP servers into the context window. For sub-agents that
only need a focused task prompt, this wastes scarce context capacity.

ACP uses JSON-RPC 2.0 over stdio with `session/new` to control initialization
context precisely:

- Inject only task-relevant skills and rules
- Load progressively on demand
- Skip default files (`CLAUDE.md`, `AGENTS.md`) when not needed
- Inject specific MCP servers per step instead of loading all

## Transport Routing

| Tool | Default Transport | Runtime Binary |
|------|-------------------|----------------|
| claude-code | ACP (`cli` opt-in) | `claude-code-acp` / `claude` |
| codex | ACP (`acp` explicit) | `codex-acp` |
| gemini-cli | CLI only | `gemini` |
| opencode | CLI only | `opencode` |

The `Transport` trait abstracts both execution modes. `TransportFactory`
routes automatically based on tool type and configuration. `claude-code`
resolves transport from the build default plus `[tools.claude-code].transport`,
probing `claude-code-acp` in ACP mode and `claude` in CLI mode. `codex`
resolves transport from `CodexRuntimeMetadata` plus `[tools.codex].transport`;
the current build default is ACP, so the default path probes `codex-acp`.
Config validation accepts codex `auto` and `acp` values without performing a
binary presence check, and `csa doctor` surfaces missing adapters and install
hints. Project config still rejects `tools.codex.transport = "cli"` today.
`gemini-cli` and `opencode` remain direct CLI tools.

To force Claude Code onto the native CLI runtime:

```toml
[tools.claude-code]
transport = "cli"
```

`csa doctor` reports the active transport and probed runtime binary so you can
verify which path CSA will use before running a session.

**Fallback rules:**

- ACP fallback to Legacy is allowed only during connection initialization
- During prompt execution, automatic fallback is forbidden
- This prevents silent degradation of context control

### Runtime verification

After changing a transport override, run `csa doctor`. It reports the active
transport and probed runtime binary for both `claude-code` and `codex`.

This matters most for codex: config validation accepts
`[tools.codex].transport = "acp"` even when `codex-acp` is absent. Missing
adapters are reported by doctor/runtime checks, not by config parsing.

## Crate API

The `csa-acp` crate provides three core abstractions:

### `AcpConnection`

Manages the underlying ACP process lifecycle:

```rust
use csa_acp::{AcpConnection, AcpConnectionOptions};

let conn = AcpConnection::spawn(AcpConnectionOptions {
    tool: ToolName::ClaudeCode,
    working_dir: project_root.clone(),
    ..Default::default()
}).await?;
```

Supports `spawn_sandboxed()` for resource-isolated ACP processes
(cgroup/rlimit integration).

### `AcpSession`

Wraps a session within an ACP connection, providing context injection:

```rust
use csa_acp::{AcpSession, SessionConfig};

let config = SessionConfig {
    no_load: vec!["CLAUDE.md".into(), "AGENTS.md".into()],
    extra_load: vec!["./rules/security.md".into()],
    mcp_servers: Some(mcp_config),
    ..Default::default()
};

let session = AcpSession::new(&conn, config).await?;
```

### `run_prompt()`

Executes a prompt within a session, streaming events:

```rust
let output = session.run_prompt("analyze auth flow", options).await?;
```

Returns `AcpOutput` with stdout, exit status, and session events.

## Context Window Control

### Controlling loaded files

```toml
# .skill.toml -- per-skill context configuration
[context]
no_load = ["CLAUDE.md", "AGENTS.md"]   # Skip default files
extra_load = ["./rules/security.md"]   # Load additional files
```

### MCP server injection

CSA's MCP registry (`.csa/mcp.toml`) supports step-level MCP server
injection. Instead of loading every MCP server from the tool's global
configuration, each workflow step can specify exactly which MCP servers
it needs.

When the MCP Hub proxy is available (`mcp_proxy_socket` exists), CSA
injects a single `csa-mcp-hub` entry that proxies all MCP requests.
Otherwise, it falls back to the direct server list.

## Session Events

ACP sessions emit `SessionEvent` objects that CSA captures for:

- **Transcripts:** JSONL event persistence via `EventWriter` in `csa-session`
- **Redaction:** Sensitive data (API keys, tokens) is automatically redacted
- **Progress tracking:** Events drive StreamMode output and idle detection

## !Send Futures

The ACP SDK uses `Rc<RefCell>` internally, making its futures `!Send`.
CSA wraps ACP operations in `tokio::task::LocalSet` inside
`spawn_blocking` with a `current_thread` runtime to handle this safely.

## Permission Model

`CsaAcpClient::request_permission` reads Agent-provided `options` and
auto-approves in yolo mode, replacing tool-specific `suppress_notify`
hacks from the Legacy CLI path.

## Exit Code Semantics

ACP processes may stay alive across multiple prompts within a session.
`unwrap_or(0)` is the correct default exit code, since the process
lifecycle is decoupled from individual prompt execution.

## Related

- [Architecture](architecture.md) -- transport routing overview
- [MCP Hub](mcp-hub.md) -- MCP proxy injection
- [Resource Control](resource-control.md) -- sandbox integration with ACP
