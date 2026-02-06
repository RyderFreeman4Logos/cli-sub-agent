# Session Management

CSA uses ULID-based sessions to track conversation history and genealogy across multiple tool invocations.

## Session Basics

### What is a Session?

A session represents a logical work context that persists across multiple tool invocations. Each session:

- Has a unique ULID identifier (26 characters, Crockford Base32)
- Tracks which tools have been used and their provider-specific session IDs
- Maintains genealogy (parent-child relationships for recursive invocations)
- Records resource usage statistics
- Supports context compression commands

### ULID Format

**Format:** 26-character Crockford Base32 string

**Example:** `01JH4QWERT1234567890ABCDEF`

**Properties:**
- Lexicographically sortable by creation time
- Timestamp-prefixed (first 10 characters)
- Collision-resistant (random component)
- Case-insensitive (base32 encoding)
- URL-safe

**Prefix Matching:**

Sessions can be referenced by prefix (similar to git commit hashes):

```bash
# Full ULID
csa run --session 01JH4QWERT1234567890ABCDEF "Continue work"

# Prefix (must be unique)
csa run --session 01JH4Q "Continue work"

# Shortest unique prefix
csa run --session 01JH "Continue work"
```

**Error Handling:**
- Ambiguous prefix (multiple matches): Lists all matching sessions
- No matches: Error message with suggestion to list sessions

## Session Lifecycle

### 1. Creation

**Automatic Creation:**

```bash
# Creates new root session (depth=0) if no --session flag
csa run "Analyze authentication module"
```

**Manual Creation:**

```bash
# Create session with description
csa session create --description "Feature X development"

# Create child session
csa session create --parent 01JH4Q --description "Subtask Y"
```

**What Happens:**
1. Generate new ULID
2. Create session directory: `~/.local/state/csa/{project_path}/sessions/{ulid}/`
3. Write initial `state.toml`
4. Return session ID to user

**Initial State:**

```toml
meta_session_id = "01JH4QWERT1234567890ABCDEF"
description = "Feature X development"
project_path = "/home/user/project"
created_at = 2024-02-06T10:00:00Z
last_accessed = 2024-02-06T10:00:00Z

[genealogy]
parent_session_id = "01JH4QWERT0000000000000000"  # Optional
depth = 1

[context_status]
is_compacted = false

[tools]
# Empty initially, populated on first tool use
```

### 2. Use

**Resume Existing Session:**

```bash
# Continue conversation in session
csa run --session 01JH4Q "Implement the changes we discussed"

# Use specific tool in session
csa run --session 01JH4Q --tool codex "Run tests"
```

**What Happens:**
1. Load `state.toml` from session directory
2. Acquire tool-level lock
3. Build command with provider session ID (if available)
4. Execute tool
5. Extract new provider session ID from output
6. Update `last_accessed` timestamp
7. Save updated state

**State After First Tool Use:**

```toml
meta_session_id = "01JH4QWERT1234567890ABCDEF"
# ... other fields ...

[tools.codex]
provider_session_id = "thread_abc123xyz"  # Codex-specific session ID
last_action_summary = "Analyzed authentication module"
last_exit_code = 0
updated_at = 2024-02-06T10:05:00Z
```

### 3. Compression

**Purpose:** Reduce context window usage in long-running sessions

**Command:**

```bash
# Compress specific session
csa session compress --session 01JH4Q

# Compress with custom instructions
csa session compress --session 01JH4Q --instructions "Keep API decisions, summarize implementation details"
```

**Tool-Specific Mapping:**

| Tool | Compression Command |
|------|---------------------|
| gemini-cli | `/compress` |
| codex | `/compact` |
| claude-code | `/compact` |
| opencode | Not supported |

**Process:**
1. Identify which tool was last used in the session
2. Map tool to compression command
3. Execute compression command in the tool's context
4. Update `context_status.is_compacted = true`
5. Set `context_status.last_compacted_at`

**State After Compression:**

```toml
[context_status]
is_compacted = true
last_compacted_at = 2024-02-06T11:00:00Z
```

### 4. Deletion

**Manual Deletion:**

```bash
# Delete specific session
csa session delete 01JH4Q

# Batch delete multiple sessions
csa session delete 01JH4Q 01JH5R 01JH6S
```

**What Happens:**
1. Validate session exists
2. Remove session directory and all contents
3. Child sessions are NOT deleted (become orphans)

**Warning:** Deletion is permanent and cannot be undone.

## Genealogy

### Parent-Child Relationships

When a tool spawns a sub-agent via CSA, a parent-child relationship is created:

```
Root Session (depth=0)
  │
  ├─ Child Session A (depth=1)
  │    │
  │    ├─ Grandchild Session A1 (depth=2)
  │    └─ Grandchild Session A2 (depth=2)
  │
  └─ Child Session B (depth=1)
```

**Tracking:**
- Parent ID stored in `genealogy.parent_session_id`
- Depth computed from parent (parent.depth + 1)
- Children discovered dynamically (not stored in parent state)

**Environment Propagation:**

When CSA spawns a tool that then invokes CSA again:

```bash
# Parent CSA process
CSA_SESSION_ID=01JH4QWERT1234567890ABCDEF
CSA_DEPTH=0
CSA_PROJECT_ROOT=/home/user/project

# ↓ Spawns gemini-cli

# Child CSA process (invoked by gemini-cli)
CSA_SESSION_ID=01JH5RNEW9876543210ZYXWVU
CSA_DEPTH=1
CSA_PARENT_SESSION=01JH4QWERT1234567890ABCDEF
CSA_PROJECT_ROOT=/home/user/project
```

### Tree Visualization

**List Sessions as Tree:**

```bash
csa session list --tree
```

**Example Output:**

```
01JH4QWERT (depth=0) - Main development session
  ├─ 01JH5RNEW (depth=1) - Refactor auth module
  │    └─ 01JH6SABC (depth=2) - Fix type errors
  └─ 01JH5RXYZ (depth=1) - Update documentation
```

**Implementation:**
1. Load all sessions
2. Build parent → children map
3. Recursively render tree starting from root sessions (depth=0)
4. Indent based on depth

### Finding Children

**CLI:**

```bash
# Find direct children of session
csa session children 01JH4Q
```

**Output:**

```
Direct children of 01JH4QWERT1234567890ABCDEF:
- 01JH5RNEW9876543210ZYXWVU (Refactor auth module)
- 01JH5RXYZ0000000000111111 (Update documentation)
```

**Implementation:**
1. Scan all sessions
2. Filter where `genealogy.parent_session_id == target_id`
3. Return list sorted by creation time

## Session Storage

### Directory Structure

```
~/.local/state/csa/
└── {project_path}/              # e.g., home/obj/project/my-app/
    ├── sessions/
    │   ├── 01JH4QWERT1234567890ABCDEF/
    │   │   ├── state.toml
    │   │   └── locks/
    │   │       ├── gemini-cli.lock
    │   │       └── codex.lock
    │   ├── 01JH5RNEW9876543210ZYXWVU/
    │   │   ├── state.toml
    │   │   └── locks/
    │   └── ...
    └── usage_stats.toml          # Shared across all sessions
```

**Key Points:**
- Sessions stored in flat directory (not nested by depth)
- Each session has independent lock directory
- Usage statistics shared at project level

### State File Format

**Path:** `{session_dir}/state.toml`

**Complete Schema:**

```toml
meta_session_id = "01JH4QWERT1234567890ABCDEF"
description = "Main development session"  # Optional
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
provider_session_id = "session_abc123"  # Optional (None on first run)
last_action_summary = "Analyzed authentication module"
last_exit_code = 0
updated_at = 2024-02-06T14:30:00Z

[tools.codex]
provider_session_id = "thread_xyz789"
last_action_summary = "Fixed type errors in auth.rs"
last_exit_code = 0
updated_at = 2024-02-06T15:00:00Z
```

**Field Descriptions:**

| Field | Type | Optional | Description |
|-------|------|----------|-------------|
| `meta_session_id` | String | No | ULID identifier |
| `description` | String | Yes | Human-readable description |
| `project_path` | String | No | Absolute path to project root |
| `created_at` | DateTime | No | ISO 8601 creation timestamp |
| `last_accessed` | DateTime | No | ISO 8601 last access timestamp |
| `genealogy.parent_session_id` | String | Yes | Parent session ULID |
| `genealogy.depth` | Integer | No | Depth in genealogy tree |
| `context_status.is_compacted` | Boolean | No | Whether context has been compressed |
| `context_status.last_compacted_at` | DateTime | Yes | When context was last compressed |
| `tools.{tool}.provider_session_id` | String | Yes | Tool-specific session ID |
| `tools.{tool}.last_action_summary` | String | No | Summary of last action |
| `tools.{tool}.last_exit_code` | Integer | No | Exit code of last invocation |
| `tools.{tool}.updated_at` | DateTime | No | When tool state was last updated |

## Locking

### Purpose

Prevent concurrent access to the same tool within a session, which could corrupt provider session state.

### Lock Granularity

**Tool-Level:** Each tool has its own lock file

**Path:** `{session_dir}/locks/{tool_name}.lock`

**Example:**
```
sessions/01JH4QWERT.../locks/
  ├── gemini-cli.lock
  ├── codex.lock
  ├── opencode.lock
  └── claude-code.lock
```

**Behavior:**
- `csa run --session X --tool gemini-cli` locks `gemini-cli.lock`
- Simultaneously, `csa run --session X --tool codex` can acquire `codex.lock`
- Second `gemini-cli` invocation in same session blocks until lock is released

### Lock Implementation

**Mechanism:** `flock` via `fd-lock` crate

**Lock Type:** Non-blocking write lock (exclusive)

**Diagnostic Information:**

Lock files contain JSON diagnostic data:

```json
{
  "pid": 12345,
  "tool_name": "gemini-cli",
  "acquired_at": "2024-02-06T10:00:00Z"
}
```

**Error Message on Lock Failure:**

```
Error: Session locked by PID 12345 (tool: gemini-cli, acquired: 2024-02-06T10:00:00Z)
```

**Lock Release:**
- Automatic on process exit (kernel-managed)
- Explicit via `SessionLock` drop (guard pattern)

### Lock Lifecycle

```rust
// Acquire lock
let lock = acquire_lock(session_dir, "gemini-cli")?;

// Lock held during tool execution
execute_tool();

// Lock automatically released when `lock` is dropped
```

**Note:** Locks are process-scoped (via `flock`), not thread-scoped.

## Ephemeral Sessions

**Definition:** Temporary sessions that don't persist to disk

**Use Case:** One-off tasks that don't need history

**Command:**

```bash
# Run with ephemeral session (not implemented yet in current codebase)
csa run --ephemeral "Quick check"
```

**Implementation Strategy:**
1. Create in-memory session state
2. Execute task in temporary directory
3. Discard all state on completion

**Current Status:** Not yet implemented (design note for future)

## Garbage Collection

### Purpose

Remove orphaned and stale sessions to reclaim disk space.

### GC Command

```bash
# Dry run (show what would be deleted)
csa gc --dry-run

# Delete sessions older than 60 days
csa gc --max-age-days 60

# Interactive confirmation
csa gc
```

### Orphan Detection

**Orphan Criteria:**

1. **Corrupt State:** Session directory exists but `state.toml` is missing or unparseable
2. **Missing Parent:** `parent_session_id` is set but parent session doesn't exist
3. **Stale:** Not accessed for > `max_age_days` (default: 30)

**Recovery for Corrupt State:**

```rust
// Backup corrupt file
state.toml → state.toml.corrupt

// Create minimal valid state
{
  meta_session_id: session_id,
  description: "(recovered from corrupt state)",
  project_path: "(unknown)",
  created_at: now,
  genealogy: { depth: 0, parent_session_id: None },
  tools: {},
}
```

### GC Process

```
1. Scan all session directories
   ↓
2. Load state.toml for each session
   ├─ Success → Add to valid sessions
   └─ Failure → Attempt recovery → Mark as orphan if unrecoverable
   ↓
3. Identify stale sessions (last_accessed > max_age_days ago)
   ↓
4. Present deletion candidates
   ↓
5. Confirm (unless --yes flag)
   ↓
6. Delete session directories
   ↓
7. Report summary (deleted count, reclaimed space)
```

## Session Listing

### Basic Listing

```bash
# List all sessions
csa session list

# Filter by tool
csa session list --tool gemini-cli

# Filter by tool (multiple)
csa session list --tool gemini-cli --tool codex
```

**Output:**

```
Session ID              | Depth | Description               | Last Accessed
------------------------|-------|---------------------------|------------------------
01JH4QWERT1234567890... | 0     | Main development session  | 2024-02-06 14:30:00 UTC
01JH5RNEW9876543210...  | 1     | Refactor auth module      | 2024-02-06 15:00:00 UTC
01JH6SABC0000000000...  | 2     | Fix type errors           | 2024-02-06 15:15:00 UTC
```

### Tree Listing

```bash
# Hierarchical view
csa session list --tree
```

**Output:**

```
Root Sessions:
  01JH4QWERT (depth=0) - Main development session
    ├─ 01JH5RNEW (depth=1) - Refactor auth module
    │    └─ 01JH6SABC (depth=2) - Fix type errors
    │    └─ 01JH6SDEF (depth=2) - Update tests
    └─ 01JH5RXYZ (depth=1) - Update documentation

  01JH7TABCD (depth=0) - Separate investigation
    └─ 01JH8UBCD (depth=1) - Prototype solution
```

### Filtering

**By Tool:**

```bash
# Sessions that have used gemini-cli at least once
csa session list --tool gemini-cli
```

**By Age:**

```bash
# Sessions created in last 7 days
csa session list --since 7d

# Sessions not accessed in 30+ days
csa session list --stale 30d
```

**By Depth:**

```bash
# Only root sessions
csa session list --depth 0

# Sessions at depth 2 or deeper
csa session list --min-depth 2
```

## Best Practices

1. **Use descriptive names:** Provide `--description` when creating sessions manually
2. **Compress long sessions:** Run `csa session compress` after 10+ interactions
3. **Clean up regularly:** Run `csa gc` monthly to remove stale sessions
4. **Prefix matching:** Use shortest unique prefix to save typing
5. **Tree view:** Use `--tree` to understand session relationships
6. **Filter by tool:** Use `--tool` to find sessions for specific tools
7. **Monitor depth:** Review sessions at depth 3+ for potential runaway recursion

## Troubleshooting

**Problem:** "Session 01JH4Q not found"

**Solution:** Session may have been deleted or ULID is incorrect. Run `csa session list` to see available sessions.

---

**Problem:** "Session locked by PID 12345"

**Solution:** Another CSA process is using the same tool in this session. Wait for it to finish or kill the process.

---

**Problem:** "Ambiguous session prefix: 01J matches 3 sessions"

**Solution:** Use a longer prefix to uniquely identify the session.

---

**Problem:** Many orphan sessions after system crash

**Solution:** Run `csa gc` to clean up orphaned sessions. Recoverable sessions will be restored with minimal state.

---

**Problem:** Context not resuming correctly after compression

**Solution:** Verify compression succeeded with `csa session list` (check `is_compacted` status). Some tools may not support resumption after compression.
