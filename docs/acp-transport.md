# ACP Transport

CSA uses the **Agent Communication Protocol (ACP)** for precise context
window control when communicating with claude-code and codex. This replaces
the CLI non-interactive mode which auto-loads 60K+ tokens of project context.

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

| Tool | Default Transport | ACP Binary |
|------|-------------------|------------|
| claude-code | ACP | `claude-code-acp` |
| codex | ACP | `codex-acp` |
| gemini-cli | Legacy CLI | N/A |
| opencode | Legacy CLI | N/A |

The `Transport` trait abstracts both execution modes. `TransportFactory`
routes automatically based on tool type and configuration.

**Fallback rules:**

- ACP fallback to Legacy is allowed only during connection initialization
- During prompt execution, automatic fallback is forbidden
- This prevents silent degradation of context control

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
