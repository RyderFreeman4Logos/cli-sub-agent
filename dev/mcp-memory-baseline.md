# MCP Memory Baseline — Phase 0 (Issue #191)

## Objective

Measure per-instance memory overhead of MCP server loading in claude-code
to determine whether an MCP wrapper daemon is justified (gate: delta >= 200 MB).

## Methodology

Two ACP session modes are compared:

| Mode | `settingSources` | MCP servers |
|------|-------------------|-------------|
| **Lean** | `[]` | None loaded |
| **Full** | `["user", "project"]` | All configured MCPs loaded |

Measurement:
- Launch via `csa run --tool claude-code` (or raw `claude-code-acp`)
- Wait for full initialization (default: 15s settle time)
- Measure process tree RSS via `/proc/<pid>/status` (VmRSS) + descendants
- Average over 3 samples per mode
- Delta = Full RSS − Lean RSS = MCP overhead per instance

## Expected Results

Based on observed MCP server behavior:

| Component | Estimated RSS |
|-----------|---------------|
| claude-code base (lean) | ~150-250 MB |
| Each MCP server (Node.js) | ~50-80 MB |
| Typical setup (3-5 MCPs) | ~150-400 MB additional |

### Projected Savings

With N concurrent `csa` instances sharing MCP servers via daemon:

| Instances | Current waste (est.) | With daemon | Savings |
|-----------|---------------------|-------------|---------|
| 3 | ~450-1200 MB | ~150-400 MB | ~300-800 MB |
| 5 | ~750-2000 MB | ~150-400 MB | ~600-1600 MB |

## Running the Measurement

```bash
# Default: 3 samples, 15s settle, project for 3 instances
./dev/measure-mcp-memory.sh

# Custom: 5 instances, 20s settle
./dev/measure-mcp-memory.sh --instances 5 --settle-secs 20
```

Output: `dev/mcp-memory-result.toml` (structured TOML report).

## Actual Results

> **Status**: PENDING — run `./dev/measure-mcp-memory.sh` to populate.
>
> Results will be written to `dev/mcp-memory-result.toml`.

## Gate Decision

| Condition | Action |
|-----------|--------|
| Delta >= 200 MB per instance | **GO** — proceed to Phase 1 (daemon design) |
| Delta < 200 MB per instance | **NO-GO** — close #191, savings insufficient to justify daemon complexity |

## Architecture Context

The MCP wrapper daemon (if approved) would:

1. Run MCP servers once in a long-lived daemon process
2. Multiplex tool calls from N concurrent claude-code instances
3. Expose MCP tools via a local socket to each ACP session
4. Eliminate N×(MCP RSS) duplication → 1×(MCP RSS) + N×(proxy overhead)

Key constraint: MCP protocol is stateless per-tool-call, so multiplexing
is straightforward. The daemon manages server lifecycle (start/stop/restart)
independently of any single AI tool session.
