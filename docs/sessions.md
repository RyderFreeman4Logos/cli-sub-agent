# Sessions

CSA sessions provide durable context across tool invocations and maintain
parent-child genealogy for recursive agent trees.

## Overview

A session represents one logical work context. Each session:

- Has a ULID identifier (26-character Crockford Base32)
- Stores tool-specific provider session IDs
- Tracks genealogy (`parent_session_id`, `depth`)
- Records access timestamps and context status
- Persists ACP transcripts as JSONL event logs

## Session IDs

CSA uses [ULID](https://github.com/ulid/spec) (Universally Unique
Lexicographically Sortable Identifier). Example: `01JH4QWERT1234567890ABCDEF`.

**Prefix matching:** You can use a unique prefix instead of the full ID,
similar to git commit hashes:

```bash
csa run --session 01JH4Q "continue work"
csa session result -s 01JK
```

If a prefix is ambiguous, CSA reports all matching sessions.

## Storage

**Location:** `~/.local/state/csa/{project_path}/sessions/{session_id}/`

Sessions use flat physical storage with logical tree structure. This
simplifies lookup, enables efficient prefix matching, and facilitates
garbage collection.

```
~/.local/state/csa/home/user/my-project/
  +-- sessions/
  |   +-- 01JH4QWERT1234.../
  |   |   +-- state.toml          # Session metadata
  |   |   +-- locks/              # Tool-level flock files
  |   |   +-- transcript.jsonl    # ACP event transcript
  |   |   +-- output/             # Execution artifacts
  |   +-- 01JH4QWERT9876.../
  |   |   +-- state.toml
  |   |   +-- ...
  +-- usage_stats.toml             # P95 memory estimates
```

## Session Lifecycle

### 1. Create (implicit)

Sessions are created automatically by `csa run` when `--session` is
not provided:

```bash
csa run --tool codex --description "Auth refactor" "analyze auth module"
```

### 2. Resume

Resume by ID, prefix, or the most recent session:

```bash
csa run --session 01JH4Q "continue implementation"
csa run --last "continue the most recent session"
```

### 3. List

```bash
csa session list                          # All sessions
csa session list --tree                   # Tree view with genealogy
csa session list --tool codex             # Filter by tool
csa session list --tool codex,claude-code # Multiple tools
csa session list --branch feat/auth       # Filter by git branch
```

### 4. Inspect

```bash
csa session result -s 01JH4Q             # Last execution result
csa session result -s 01JH4Q --json      # JSON output
csa session logs -s 01JH4Q               # View session logs
csa session logs -s 01JH4Q --tail 200    # Last 200 lines
csa session artifacts -s 01JH4Q          # List output artifacts
csa session log -s 01JH4Q               # Git history for session
```

### 5. Compress

Send the tool-specific compaction command:

| Tool | Compression Command |
|------|---------------------|
| gemini-cli | `/compress` |
| codex | `/compact` |
| claude-code | `/compact` |
| opencode | (not supported) |

```bash
csa session compress --session 01JH4Q
```

### 6. Liveness Check

Check whether a session process is still running:

```bash
csa session is-alive --session 01JH4Q
```

Uses filesystem liveness signals (lock files and heartbeat timestamps).

### 7. Checkpoint

Write audit checkpoints via git-notes:

```bash
csa session checkpoint --session 01JH4Q   # Create checkpoint
csa session checkpoints                   # List all checkpoints
```

Checkpoints persist audit snapshots bound to each session commit,
enabling post-hoc audit trail inspection.

### 8. Delete

```bash
csa session delete --session 01JH4Q
```

### 9. Clean

Remove sessions not accessed within N days:

```bash
csa session clean --days 30
csa session clean --days 30 --dry-run
csa session clean --days 30 --tool codex
```

## Genealogy

CSA records parent-child relationships in each session's `state.toml`:

```toml
[genealogy]
parent_session_id = "01JH4QWERT0000000000000000"
depth = 1
```

When CSA spawns a sub-agent, it sets environment variables for the child:

| Variable | Description |
|----------|-------------|
| `CSA_SESSION_ID` | Current session ULID |
| `CSA_DEPTH` | Recursion depth (0 = root) |
| `CSA_PROJECT_ROOT` | Absolute project path |
| `CSA_PARENT_SESSION` | Parent session ULID |
| `CSA_SESSION_DIR` | Absolute session directory path |

View the tree:

```bash
csa session list --tree
```

## State File

`state.toml` contains the complete session metadata:

```toml
meta_session_id = "01JH4QWERT1234567890ABCDEF"
description = "Auth refactor"
project_path = "/home/user/project"
created_at = 2024-02-06T10:00:00Z
last_accessed = 2024-02-06T14:30:00Z

[genealogy]
parent_session_id = "01JH4QWERT0000000000000000"
depth = 1

[context_status]
is_compacted = false
last_compacted_at = "2024-02-06T12:00:00Z"

[tools.codex]
provider_session_id = "thread_abc123"
last_action_summary = "Reviewed auth flow"
last_exit_code = 0
updated_at = 2024-02-06T14:30:00Z
```

## Transcripts

ACP sessions emit events that are persisted as JSONL transcripts:

- **File:** `{session_dir}/transcript.jsonl`
- **Schema:** Versioned (`v: 1`), with sequential `seq` numbers
- **Redaction:** Sensitive data (API keys, tokens) is automatically redacted
- **Resume-safe:** Transcript writer detects existing lines and continues
  from the correct sequence number
- **Atomic writes:** Buffered with periodic flush, truncates partial
  trailing lines on recovery

## Ephemeral Sessions

For one-off tasks that don't need persistence:

```bash
csa run --ephemeral "quick question about syntax"
```

Ephemeral sessions skip project file loading, context injection, and
are automatically cleaned up after completion.

## Session State Machine

```
Active --> Available (after compression) --> Retired (after GC)
```

- **Active:** Session is in use or recently used
- **Available:** Session context has been compressed, still accessible
- **Retired:** Session marked for garbage collection

## Troubleshooting

| Problem | Solution |
|---------|----------|
| "No sessions found" | Run `csa run` first to create a session |
| "Session prefix is ambiguous" | Use a longer prefix or full ULID |
| "Session locked by PID ..." | Another process is using the session; retry later |

## Related

- [Architecture](architecture.md) -- flat storage design
- [Commands](commands.md) -- `csa session` reference
- [ACP Transport](acp-transport.md) -- transcript event sources
