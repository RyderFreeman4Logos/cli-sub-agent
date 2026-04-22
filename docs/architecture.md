# Architecture

CSA is a recursive agent container built as a Rust workspace with 14 crates,
each encapsulating a distinct domain concern.

## Design Principles

### Fractal Recursion

Any agent running inside CSA can spawn sub-agents by invoking `csa` again.
This creates a tree of independent Unix processes:

```
csa run (depth=0, claude-code)
  +-- csa run (depth=1, codex)          # review sub-agent
  |   +-- csa run (depth=2, gemini)     # deep analysis
  +-- csa run (depth=1, codex)          # debate sub-agent
      +-- csa run (depth=2, claude)     # adversary
```

**Recursion control:**

- Maximum depth is configurable via `max_recursion_depth` (default: 5)
- Tracked via `CSA_DEPTH` environment variable, incremented per level
- Sub-agents cannot operate on parent sessions (enforced isolation)

### Flat Storage, Logical Tree

Sessions are stored physically at the same level but maintain a logical
parent-child tree through genealogy metadata:

```
~/.local/state/csa/{project_path}/sessions/
  +-- 01JH4QWERT1234.../ (depth=0, root)
  |   +-- state.toml
  +-- 01JH4QWERT9876.../ (depth=1, parent=01JH4Q...)
  |   +-- state.toml
  +-- ...
```

**Why flat?** Simplifies ULID lookup, enables prefix matching, avoids deep
nesting, and makes garbage collection straightforward.

### Closed Enum for Tools

CSA uses a closed enum (`Executor`) for the four supported tools rather than
trait-based polymorphism:

```rust
pub enum Executor {
    GeminiCli { model_override, thinking_budget },
    Opencode  { model_override, agent, thinking_budget },
    Codex     { model_override, thinking_budget },
    ClaudeCode { model_override, thinking_budget },
}
```

**Rationale:** Fixed tool set, direct pattern matching, compile-time
exhaustiveness, zero vtable overhead.

### Heterogeneous Execution

CSA detects the parent tool via `/proc` filesystem inspection
(`detect_parent_tool()` in `run_helpers.rs`). In `--tool auto` mode, it
selects a tool from a **different model family** than the parent. If no
heterogeneous tool is available, CSA fails with an explicit error rather
than silently degrading.

| Parent Tool | Auto-selected Review Tool |
|-------------|--------------------------|
| claude-code | codex or gemini-cli |
| codex | claude-code or gemini-cli |
| gemini-cli | claude-code or codex |

## Crate Structure

```
crates/
  +-- cli-sub-agent/   # Main CLI binary (csa)
  +-- csa-core/        # Core types: ToolName, ULID, OutputFormat, ConsensusStrategy
  +-- csa-acp/         # ACP transport: AcpConnection, AcpSession, run_prompt()
  +-- csa-session/     # Session CRUD, genealogy, transcripts, event writer
  +-- csa-executor/    # Tool executor: closed enum, Transport trait
  +-- csa-process/     # Process spawning, setsid, signals, sandbox integration
  +-- csa-config/      # Config loading: global + project merge, migrations, registry
  +-- csa-resource/    # ResourceGuard, MemoryMonitor, cgroup, rlimit, sandbox
  +-- csa-scheduler/   # Tier rotation, 429 failover, concurrency slot management
  +-- csa-mcp-hub/     # MCP server fan-out daemon, FIFO queue, stateful pooling
  +-- csa-hooks/       # Lifecycle hooks (pre_run, post_run, etc.) and prompt guards
  +-- csa-todo/        # Git-tracked TODO/plan management with DAG visualization
  +-- csa-lock/        # flock-based locking (session locks, global slot locks)
  +-- weave/           # skill-lang compiler: parse, compile, execute (weave binary)
```

### Dependency Graph

```
cli-sub-agent
  +-> csa-config
  +-> csa-core
  +-> csa-executor
  |     +-> csa-core
  |     +-> csa-process
  |     +-> csa-session
  +-> csa-acp
  +-> csa-lock (independent, no internal deps)
  +-> csa-process
  +-> csa-resource
  |     +-> csa-core
  +-> csa-scheduler
  |     +-> csa-config
  |     +-> csa-session
  +-> csa-session
  |     +-> csa-core
  |     +-> csa-acp
  +-> csa-hooks
  +-> csa-todo
  +-> csa-mcp-hub
  +-> weave
```

`csa-lock` is intentionally independent with zero internal dependencies,
making it reusable outside the CSA workspace.

## Data Flow

### Command Execution

```
User -> csa run "prompt"
  |
  +-> Load config (global + project merge)
  +-> Resolve tool / tier / alias
  +-> Create or load session
  +-> Pre-flight resource check (P95 memory estimation)
  +-> Acquire session lock (flock)
  +-> Resolve runtime metadata / select transport (ACP or Legacy)
  |
  +-> [ACP path]
  |   +-> AcpConnection::spawn() -> child process
  |   +-> AcpSession::new() with SessionConfig (context injection)
  |   +-> AcpSession::run_prompt() -> stream events
  |
  +-> [Legacy path]
  |   +-> Build tool command with yolo flags
  |   +-> tokio::process::Command::spawn()
  |
  +-> MemoryMonitor: sample RSS every 500ms, track peak
  +-> StreamMode: tee stdout to stderr (TTY default)
  +-> Wait for completion
  +-> Update session state.toml
  +-> Record peak memory in usage_stats.toml
  +-> Fire hooks: post_run -> session_complete
  +-> Return result
```

### Transport Routing

| Tool | Transport | Runtime Binary |
|------|-----------|----------------|
| claude-code | ACP by default, CLI opt-in | `claude-code-acp` / `claude` |
| codex | ACP by default; config currently accepts `auto` / `acp` | `codex-acp` |
| gemini-cli | CLI only | `gemini` |
| opencode | CLI only | `opencode` |

The `Transport` trait abstracts both modes. `TransportFactory` routes based
on tool type and config. For `claude-code`, CSA resolves
`[tools.claude-code].transport`, routes `cli` to `LegacyTransport`, and routes
`acp` to `AcpTransport`; without an override, the current build default stays
on ACP. For `codex`, CSA resolves `CodexRuntimeMetadata::default_for_build()`
plus `[tools.codex].transport`; the current build default is ACP, so the
default path probes `codex-acp`. Config validation currently accepts codex
`auto` and `acp` without checking whether the adapter binary is installed, and
`csa doctor` surfaces missing adapters plus install hints. Project config
still rejects codex `cli` overrides today. `gemini-cli` and `opencode` stay
on their direct CLI binaries. ACP fallback to Legacy is allowed only during
connection initialization;
during prompt execution, automatic fallback is forbidden.

`csa doctor` makes this routing visible per tool by reporting the active
transport and probed runtime binary for both `codex` and `claude-code`.
Codex additionally reports whether ACP support was compiled in.

## Environment Variables

CSA propagates session context to child processes:

| Variable | Description |
|----------|-------------|
| `CSA_SESSION_ID` | Current session ULID |
| `CSA_DEPTH` | Recursion depth (0 = root) |
| `CSA_PROJECT_ROOT` | Absolute project directory path |
| `CSA_PARENT_SESSION` | Parent session ULID (optional) |
| `CSA_TOOL` | Current tool name |
| `CSA_PARENT_TOOL` | Parent's tool name |
| `CSA_SESSION_DIR` | Absolute path to session directory |

CSA automatically strips `CLAUDECODE` and `CLAUDE_CODE_ENTRYPOINT` when
spawning child processes, so no manual `env -u` prefix is needed.

## Process Model

### Process Group Isolation

Child processes run in separate process groups via `setsid()`. This prevents
signal inheritance from the parent and enables clean termination of the
entire subprocess tree.

### Signal Handling

- `SIGTERM` and `SIGINT` propagate to child process groups
- `kill_on_drop` enabled as safety net
- Two-phase termination: SIGTERM first, SIGKILL after configurable grace period

### Yolo Mode

All tools run with automatic approvals for non-interactive sub-agent execution:

| Tool | Yolo Flag |
|------|-----------|
| gemini-cli | `-y` |
| codex | `--dangerously-bypass-approvals-and-sandbox` |
| claude-code | `--dangerously-skip-permissions` |
| opencode | (non-interactive by design) |

## Garbage Collection

`csa gc` removes orphaned and stale sessions:

- **Orphan detection:** sessions with missing `state.toml` or broken parent refs
- **Staleness:** sessions not accessed within N days (default: 30)
- **Transcript GC:** expired JSONL transcripts are cleaned alongside sessions
- **Dry-run:** `csa gc --dry-run` shows what would be removed
- **Global:** `csa gc --global` scans all projects under `~/.local/state/csa/`
