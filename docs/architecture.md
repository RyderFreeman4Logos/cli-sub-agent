# Architecture Overview

CSA (CLI Sub-Agent) is a recursive agent container designed to orchestrate multiple AI CLI tools with session management, resource control, and process isolation.

## Core Design Principles

### 1. Fractal & Recursive

CSA implements a recursive agent model where any agent running inside CSA can spawn sub-agents by invoking CSA again. This creates a fractal tree structure:

```
csa (depth=0)
  └─ gemini-cli task A
       └─ csa (depth=1) spawned by gemini-cli
            └─ codex subtask A1
                 └─ csa (depth=2)
                      └─ ...
```

**Recursion Control:**
- Maximum recursion depth: Configurable via `max_recursion_depth` in config (default: 5)
- Depth tracking: `CSA_DEPTH` environment variable propagated to child processes
- Safety: Sub-agents cannot operate on parent sessions (enforced isolation)

### 2. Flat Physical Storage, Logical Tree Structure

**Storage Location:** `~/.local/state/csa/{project_path}/sessions/{session_id}/`

Sessions are stored physically at the same level (flat directory structure) but maintain logical parent-child relationships through genealogy metadata.

```
~/.local/state/csa/
└── home/obj/project/my-app/
    └── sessions/
        ├── 01JH4QWERT1234567890ABCDEF/    # Root session (depth=0)
        │   ├── state.toml
        │   └── locks/
        ├── 01JH4QWERT9876543210ZYXWVU/    # Child session (depth=1)
        │   ├── state.toml
        │   └── locks/
        └── ...
```

**Why Flat Storage:**
- Simplifies session lookup by ULID
- Avoids deep nesting issues
- Enables efficient prefix matching
- Facilitates garbage collection

### 3. Session Isolation & Locking

**Tool-Level Locking:**
- Each tool within a session has its own lock file
- Lock path: `{session_dir}/locks/{tool_name}.lock`
- Implementation: `flock` via `fd-lock` crate (non-blocking write locks)
- Diagnostic info: Lock files contain JSON with PID, tool name, and acquisition timestamp

**Isolation Guarantees:**
- Sub-agents cannot access parent session state
- Each session has independent tool state
- Concurrent runs of different tools in the same session are prevented per-tool

## Crate Structure

CSA is organized into 9 crates following domain separation:

```
crates/
├── cli-sub-agent/      # Main CLI binary and orchestration
├── csa-config/         # Configuration management (TOML-based)
├── csa-core/           # Core types and utilities
├── csa-executor/       # Tool execution (4-variant enum)
├── csa-lock/           # File-based locking (flock)
├── csa-process/        # Process spawning and signal handling
├── csa-resource/       # Memory monitoring and scheduling
├── csa-scheduler/      # Tier rotation and 429 failover decisions
└── csa-session/        # Session CRUD and genealogy tracking
```

### Dependency Graph

```
cli-sub-agent
    ├─> csa-config
    ├─> csa-core
    ├─> csa-executor
    │     ├─> csa-core
    │     ├─> csa-process
    │     └─> csa-session
    ├─> csa-lock (independent)
    ├─> csa-process
    ├─> csa-resource
    │     └─> csa-core (optional)
    ├─> csa-scheduler
    │     ├─> csa-config
    │     └─> csa-session
    └─> csa-session
          └─> csa-core (optional)
```

**Design Note:** `csa-lock` is intentionally independent with no internal CSA dependencies, making it reusable.

## Data Flow

### 1. CLI Invocation

```
User
  │
  ├─> csa run --tool gemini-cli "analyze code"
  │
  ▼
cli-sub-agent
  │
  ├─> Load config (.csa/config.toml)
  ├─> Resolve tool/tier/alias
  ├─> Create or load session
  ├─> Check resource availability
  │
  ▼
Executor (enum)
  │
  ├─> Build command with environment
  ├─> Acquire lock
  ├─> Spawn child process
  │
  ▼
Child Process (gemini/codex/opencode/claude)
  │
  ├─> Execute task (with yolo mode)
  ├─> Write to stdout (captured)
  │
  ▼
Process Monitor
  │
  ├─> Track peak memory
  ├─> Wait for completion
  ├─> Extract session ID from output
  │
  ▼
Session Update
  │
  ├─> Update tool state
  ├─> Record resource usage
  ├─> Save state.toml
  │
  ▼
Return Result
```

### 2. Session State Management

**State File:** `{session_dir}/state.toml`

```toml
meta_session_id = "01JH4QWERT1234567890ABCDEF"
description = "Main development session"
project_path = "/home/user/project"
created_at = 2024-02-06T10:00:00Z
last_accessed = 2024-02-06T14:30:00Z

[genealogy]
parent_session_id = "01JH4QWERT0000000000000000"  # Optional
depth = 1

[context_status]
is_compacted = false
last_compacted_at = "2024-02-06T12:00:00Z"  # Optional

[tools.gemini-cli]
provider_session_id = "session_abc123"
last_action_summary = "Analyzed authentication module"
last_exit_code = 0
updated_at = 2024-02-06T14:30:00Z
```

### 3. Resource Monitoring Flow

```
Pre-Flight Check
  │
  ├─> Load usage_stats.toml
  ├─> Calculate P95 memory estimate
  ├─> Check available RAM + swap
  │
  ├─ [Insufficient] ──> Abort with OOM risk message
  │
  └─ [Sufficient] ──> Continue
           │
           ▼
     Spawn Tool
           │
           ├─> Get child PID
           ├─> Monitor memory usage
           ├─> Record peak memory
           │
           ▼
     Wait for Completion
           │
           ├─> Extract exit code
           ├─> Update usage_stats.toml
           └─> Return result
```

**Stats File:** `{session_dir}/usage_stats.toml`

```toml
[history]
gemini-cli = [1024, 1536, 1280, 1792, ...]  # Last 20 runs (MB)
codex = [2048, 2304, 2176, ...]
```

## Process Model

### 1. Process Isolation

**Process Group Isolation:**
- Child processes run in separate process groups via `setsid()`
- Prevents signal inheritance from parent
- Enables clean termination of entire subprocess tree

**Signal Handling:**
- `SIGTERM` and `SIGINT` propagated to child process groups
- `kill_on_drop` enabled as safety net
- Ensures no zombie processes

### 2. Yolo Mode

All tools run with automatic approvals to enable non-interactive sub-agent execution:

| Tool | Yolo Flag |
|------|-----------|
| gemini-cli | `-y` |
| codex | `--dangerously-bypass-approvals-and-sandbox` |
| claude-code | `--dangerously-skip-permissions` |
| opencode | (none, non-interactive by design) |

**Safety Trade-off:** Yolo mode is necessary for sub-agents but requires careful prompt construction and validation at the orchestration layer.

## Environment Variables

CSA propagates session context via environment variables:

| Variable | Value | Purpose |
|----------|-------|---------|
| `CSA_SESSION_ID` | ULID | Current session identifier |
| `CSA_DEPTH` | Integer | Recursion depth (0 = root) |
| `CSA_PROJECT_ROOT` | Absolute path | Project directory |
| `CSA_PARENT_SESSION` | ULID | Parent session ID (optional) |

**Usage by Child Tools:**
- Tools can read `CSA_DEPTH` to self-limit recursion
- `CSA_SESSION_ID` enables session resumption
- `CSA_PROJECT_ROOT` provides workspace context

## Executor Architecture

### Closed Enum Design

Rather than using trait-based polymorphism, CSA uses a closed enum for the 4 supported tools:

```rust
pub enum Executor {
    GeminiCli { model_override: Option<String>, thinking_budget: Option<ThinkingBudget> },
    Opencode { model_override: Option<String>, agent: Option<String>, thinking_budget: Option<ThinkingBudget> },
    Codex { model_override: Option<String>, thinking_budget: Option<ThinkingBudget> },
    ClaudeCode { model_override: Option<String>, thinking_budget: Option<ThinkingBudget> },
}
```

**Rationale:**
- Fixed set of tools (not extensible at runtime)
- Direct pattern matching (better than dynamic dispatch for this case)
- Compile-time exhaustiveness checking
- Zero overhead (no vtables)

### Command Building

Each executor variant implements tool-specific command construction:

```rust
impl Executor {
    pub fn build_command(&self, prompt: &str, tool_state: Option<&ToolState>, session: &MetaSessionState) -> Command {
        let mut cmd = self.build_base_command(session);  // Set env vars, working dir
        self.append_tool_args(&mut cmd, prompt, tool_state);  // Tool-specific args
        cmd
    }
}
```

**Separation of Concerns:**
- Base command: Session environment setup
- Tool args: Provider-specific flags and session resumption
- Restrictions: Applied via prompt modification (not args)

## Context Compression Mapping

Different tools have different compression commands:

| Tool | Compression Command |
|------|---------------------|
| gemini-cli | `/compress` |
| codex | `/compact` |
| claude-code | `/compact` |
| opencode | (not supported) |

**Implementation:** Session manager maps tool name to appropriate compression command when `csa session compress` is invoked.

## Garbage Collection

CSA implements garbage collection for orphaned and stale sessions:

**Orphan Detection:**
- Sessions with missing or corrupt `state.toml`
- Sessions with no parent when `parent_session_id` is set but parent doesn't exist

**Staleness Heuristic:**
- Sessions not accessed for > 30 days (configurable)
- Sessions with depth > 0 and no recent activity

**GC Process:**
1. Scan all session directories
2. Load each `state.toml` (recover corrupt files if possible)
3. Identify orphans and stale sessions
4. Optionally delete (with confirmation)

**Command:** `csa gc [--dry-run] [--max-age-days N]`
