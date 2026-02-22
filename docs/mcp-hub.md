# MCP Hub

CSA includes a shared **MCP Hub daemon** (`csa mcp-hub`) that provides
fan-out multiplexing of MCP (Model Context Protocol) servers over a
Unix domain socket. This enables multiple concurrent agents to share
a single pool of MCP server instances.

## Overview

Without MCP Hub, each agent process launches its own set of MCP servers,
leading to redundant memory usage and startup delays. The MCP Hub
centralizes server management:

```
Agent-1 --+
Agent-2 --+--> csa-mcp-hub (Unix socket) --> MCP Server A
Agent-3 --+                              --> MCP Server B
                                         --> MCP Server C
```

## Commands

### Start the hub

```bash
# Foreground (default)
csa mcp-hub serve

# Background daemon
csa mcp-hub serve --background

# Custom socket path
csa mcp-hub serve --socket /tmp/my-hub.sock

# Linux systemd socket activation
csa mcp-hub serve --systemd-activation
```

### Check status

```bash
csa mcp-hub status [--socket <PATH>]
```

### Stop the hub

```bash
csa mcp-hub stop [--socket <PATH>]
```

### Generate routing-guide skill

```bash
csa mcp-hub gen-skill [--socket <PATH>]
```

Generates a `.claude/skills/mcp-hub-routing-guide/` directory from the
live `tools/list` response. The skill uses progressive disclosure:
`SKILL.md` (overview) -> `references/` -> `mcps/<name>.md` (per-server
details). It auto-refreshes when `tools/list_changed` is signaled.

## Socket Path

The default socket path follows XDG conventions:

1. `$XDG_RUNTIME_DIR/cli-sub-agent/mcp-hub.sock` (if `$XDG_RUNTIME_DIR` set)
2. `/tmp/cli-sub-agent-$UID/mcp-hub.sock` (fallback)

Override globally via `mcp_proxy_socket` in
`~/.config/cli-sub-agent/config.toml`.

## FIFO Queue

Each MCP server gets a bounded FIFO dispatch queue
(`REQUEST_QUEUE_CAPACITY=64`) that prevents head-of-line starvation.
Features:

- **Bounded capacity:** Back-pressure when queue is full
- **Cancellation-aware:** Dequeue skips requests whose callers have disconnected
- **Per-server isolation:** Slow server does not block requests to other servers

## Stateful Pooling

Stateful MCP servers (those maintaining internal state across requests)
use pool keys composed of `project_root` + `toolchain_hash`:

- **Warm TTL reuse:** Pool instances are reused within their TTL
- **Lease tracking:** Active leases prevent premature eviction
- **Pressure reclaim:** `max_warm_pools` triggers LRU eviction under pressure
- **Hard guard:** `max_active_pools` prevents unbounded pool growth

## Proxy Injection

When an ACP session is created, CSA checks for a running MCP Hub:

1. If `mcp_proxy_socket` exists -> inject single `csa-mcp-hub` entry
2. Otherwise -> inject the direct MCP server list from config

This is transparent to the agent: MCP tool calls route through the hub
without any code changes.

## Service Integration

### Linux (systemd)

The project includes systemd user units:

- `systemd/mcp-hub.socket` -- socket activation unit
- `systemd/mcp-hub.service` -- service unit

```bash
systemctl --user enable csa-mcp-hub.socket
systemctl --user start csa-mcp-hub.socket
```

### macOS (launchd)

See [MCP Hub on macOS](mcp-hub-launchd.md) for launchd plist configuration.

## Memory Measurement

The `dev/measure-mcp-memory.sh` script records projected concurrent RSS:

```bash
dev/measure-mcp-memory.sh --proxy [--proxy-socket PATH]
```

Reports projected memory usage with a `< 4GB` check.

## Crate Structure

The `csa-mcp-hub` crate contains:

| Module | Purpose |
|--------|---------|
| `serve` | Hub lifecycle (serve, status, stop, gen-skill commands) |
| `registry` | MCP server registry and tool discovery |
| `proxy` | Request proxying and fan-out dispatch |
| `config` | Hub-specific configuration loading |
| `skill_writer` | Routing-guide skill generation |
| `socket` | Unix domain socket management |

## Related

- [ACP Transport](acp-transport.md) -- MCP proxy injection in ACP sessions
- [Configuration](configuration.md) -- `mcp_proxy_socket` setting
- [MCP Hub on macOS](mcp-hub-launchd.md) -- launchd integration
