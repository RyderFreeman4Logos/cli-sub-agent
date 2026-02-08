# Session Management

CSA sessions provide durable context across tool invocations and keep parent-child genealogy for recursive runs.

## Overview

A session represents one logical work context. Each session:
- has a ULID id
- stores tool-specific provider session ids
- tracks genealogy (`parent_session_id`, `depth`)
- records access and context status

Session state is stored under:
- `~/.local/state/csa/{project_path}/sessions/{session_id}/state.toml`

## Session ID Format

CSA uses 26-character ULIDs (Crockford Base32), for example:
- `01JH4QWERT1234567890ABCDEF`

You can pass a unique prefix instead of the full id:

```bash
csa run --session 01JH4Q "Continue work"
```

If a prefix is ambiguous, CSA reports matching sessions.

## Session Lifecycle

### 1) Create (Implicit)

Sessions are created by `csa run` when `--session` is not provided.

```bash
csa run --tool codex --description "Auth refactor" "Analyze auth module"
```

### 2) Resume

Resume by id/prefix or use the latest session.

```bash
csa run --session 01JH4Q "Continue implementation"
csa run --last "Continue the most recent session"
```

### 3) List

List sessions, optionally as a tree and filtered by tool.

```bash
csa session list
csa session list --tree
csa session list --tool codex
csa session list --tool codex,claude-code
```

### 4) Compress

Send the tool-specific compaction command in-session:
- `gemini-cli` -> `/compress`
- `codex` / `claude-code` -> `/compact`

```bash
csa session compress --session 01JH4Q
```

### 5) Delete

Delete one session by id or prefix.

```bash
csa session delete --session 01JH4Q
```

### 6) Clean Old Sessions

Remove sessions not accessed within N days.

```bash
csa session clean --days 30
csa session clean --days 30 --dry-run
csa session clean --days 30 --tool codex
```

### 7) Logs

Inspect session log output.

```bash
csa session logs --session 01JH4Q
csa session logs --session 01JH4Q --tail 200
```

## Genealogy and Recursion

CSA records parent-child relationships in each session state:
- `genealogy.parent_session_id`
- `genealogy.depth`

A typical tree can be viewed with:

```bash
csa session list --tree
```

When CSA launches a tool process, it sets context variables:
- `CSA_SESSION_ID`
- `CSA_DEPTH`
- `CSA_PROJECT_ROOT`
- `CSA_PARENT_SESSION` (when present)

## State File Shape

A typical `state.toml` includes:
- `meta_session_id`
- `description`
- `project_path`
- timestamps (`created_at`, `last_accessed`)
- `[genealogy]`
- `[context_status]`
- `[tools.<tool-name>]`

Example snippet:

```toml
meta_session_id = "01JH4QWERT1234567890ABCDEF"
description = "Auth refactor"
project_path = "/home/user/project"

[genealogy]
parent_session_id = "01JH4QWERT0000000000000000"
depth = 1

[context_status]
is_compacted = false

[tools.codex]
provider_session_id = "thread_abc123"
last_action_summary = "Reviewed auth flow"
last_exit_code = 0
```

## Troubleshooting

- `No sessions found`:
  Run `csa run ...` first to create a session.

- `Session prefix is ambiguous`:
  Use a longer prefix or full ULID from `csa session list`.

- `Session locked by PID ...`:
  Another process is using that tool/session. Retry later or use another session.
