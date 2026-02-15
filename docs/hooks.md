# Hooks Reference

This document lists all supported hook events and the template variables available for each event.

Template variables use `{name}` syntax in `hooks.toml` command strings. CSA shell-escapes substituted values. Unknown placeholders are left unchanged.

## `pre_run`

Fires before a tool execution starts in `csa run`.

Available variables:

| Variable | Description |
|---|---|
| `{session_id}` | Session ULID (`meta_session_id`) |
| `{session_dir}` | Absolute path to this session directory |
| `{sessions_root}` | Parent directory containing all sessions for this project |
| `{tool}` | Tool name for this execution (`codex`, `claude-code`, etc.) |

Example:

```toml
[pre_run]
enabled = true
command = "echo pre-run: session={session_id} tool={tool} dir={session_dir}"
timeout_secs = 30
```

## `post_run`

Fires after tool execution completes and session state has been saved.

Available variables:

| Variable | Description |
|---|---|
| `{session_id}` | Session ULID (`meta_session_id`) |
| `{session_dir}` | Absolute path to this session directory |
| `{sessions_root}` | Parent directory containing all sessions for this project |
| `{tool}` | Tool name for this execution (`codex`, `claude-code`, etc.) |
| `{exit_code}` | Tool process exit code |

Example:

```toml
[post_run]
enabled = true
command = "echo post-run: session={session_id} exit={exit_code}"
timeout_secs = 30
```

## `session_complete`

Fires after `post_run`, at the end of `csa run` execution.

Available variables:

| Variable | Description |
|---|---|
| `{session_id}` | Session ULID (`meta_session_id`) |
| `{session_dir}` | Absolute path to this session directory |
| `{sessions_root}` | Parent directory containing all sessions for this project |
| `{tool}` | Tool name for this execution (`codex`, `claude-code`, etc.) |
| `{exit_code}` | Tool process exit code |

Built-in default command (used when `[session_complete].command` is not set):

```toml
[session_complete]
enabled = true
command = "cd {sessions_root} && git add {session_id}/ && git commit -m 'session {session_id} complete' -q --allow-empty"
timeout_secs = 30
```

## `todo_create`

Fires after `csa todo create` has created a plan and committed it to the TODO git repository.

Available variables:

| Variable | Description |
|---|---|
| `{plan_id}` | TODO plan ULID/timestamp |
| `{plan_dir}` | Absolute path to the plan directory |
| `{todo_root}` | Absolute path to TODO repository root |

Example:

```toml
[todo_create]
enabled = true
command = "echo todo-create: plan={plan_id} dir={plan_dir}"
timeout_secs = 30
```

## `todo_save`

Fires after `csa todo save` has committed updated plan content (only when there are changes to save).

Available variables:

| Variable | Description |
|---|---|
| `{plan_id}` | TODO plan ULID/timestamp |
| `{plan_dir}` | Absolute path to the plan directory |
| `{todo_root}` | Absolute path to TODO repository root |
| `{version}` | Number of saved versions for this plan after save |
| `{message}` | Commit message used for this save |

Example:

```toml
[todo_save]
enabled = true
command = "echo todo-save: plan={plan_id} version={version} msg={message}"
timeout_secs = 30
```

## `prompt_guard`

Prompt guards are user-configurable shell scripts that inject text into the tool's prompt before execution. Unlike regular hooks (fire-and-forget), prompt guards **capture stdout** and append it to the `effective_prompt` as `<prompt-guard>` XML blocks.

This enables "reverse prompt injection" — reminding tools (including those without native hook systems like codex, opencode, gemini-cli) to follow AGENTS.md rules such as branch protection, timely commits, and timely PRs.

### How it works

1. CSA loads `[[prompt_guard]]` entries from `hooks.toml`
2. Before tool execution, CSA runs each guard script sequentially
3. Each script receives a JSON context on **stdin** and writes injection text to **stdout**
4. Non-empty stdout is wrapped in `<prompt-guard name="...">` XML and appended to the prompt
5. Non-zero exit or timeout = warn + skip (never blocks execution)

### Guard context (stdin JSON)

Each guard script receives the following JSON object on stdin:

```json
{
  "project_root": "/path/to/project",
  "session_id": "01ABCDEF...",
  "tool": "codex",
  "is_resume": false,
  "cwd": "/path/to/project"
}
```

| Field | Type | Description |
|---|---|---|
| `project_root` | string | Absolute path to the project root |
| `session_id` | string | Current session ULID |
| `tool` | string | Tool being executed (`codex`, `claude-code`, `gemini-cli`, `opencode`) |
| `is_resume` | bool | `true` if resuming a session (`--session` / `--last`) |
| `cwd` | string | Current working directory |

### Script protocol

| Aspect | Behavior |
|---|---|
| **stdin** | JSON `GuardContext` |
| **stdout** | Injection text (empty = no injection) |
| **stderr** | Ignored |
| **exit 0** | Success — stdout captured |
| **exit non-zero** | Warning logged, guard skipped |
| **timeout** | Warning logged, guard killed and skipped |

### Configuration

```toml
[[prompt_guard]]
name = "branch-protection"
command = "/path/to/guard-branch.sh"
timeout_secs = 5

[[prompt_guard]]
name = "commit-reminder"
command = "/path/to/remind-commit.sh"
timeout_secs = 10
```

| Field | Type | Default | Description |
|---|---|---|---|
| `name` | string | required | Human-readable name (used in XML tag and logs) |
| `command` | string | required | Shell command (run via `sh -c`) |
| `timeout_secs` | integer | 10 | Max execution time in seconds |

### Execution order and merge

- Guards execute in array order (first `[[prompt_guard]]` entry runs first)
- Config merge: project-level `[[prompt_guard]]` **replaces** global-level entirely (non-empty wins)
- Multiple guards produce multiple `<prompt-guard>` blocks in the prompt

### Output format

Guard output is injected as XML blocks in the effective prompt:

```xml
<prompt-guard name="branch-protection">
You are on branch main. Do NOT commit directly to this branch.
Create a feature branch first: git checkout -b feat/description
</prompt-guard>
<prompt-guard name="commit-reminder">
You have 3 uncommitted files. Remember to commit your work.
</prompt-guard>
```

### Example guard scripts

#### Branch protection guard

```bash
#!/bin/sh
# guard-branch.sh — Warn when on protected branches
# Reads GuardContext JSON from stdin, outputs warning text on stdout

set -e

# Parse project_root from stdin JSON
CONTEXT=$(cat)
PROJECT_ROOT=$(echo "$CONTEXT" | jq -r '.project_root')

cd "$PROJECT_ROOT" 2>/dev/null || exit 0

BRANCH=$(git branch --show-current 2>/dev/null) || exit 0

case "$BRANCH" in
  main|master|dev|develop)
    echo "WARNING: You are on protected branch '$BRANCH'."
    echo "Do NOT commit directly. Create a feature branch first:"
    echo "  git checkout -b feat/<description>"
    echo "  git checkout -b fix/<description>"
    ;;
esac
# Empty stdout on non-protected branches = no injection
```

#### Commit reminder guard

```bash
#!/bin/sh
# remind-commit.sh — Remind about uncommitted changes and unpushed commits
# Reads GuardContext JSON from stdin, outputs reminder text on stdout

set -e

CONTEXT=$(cat)
PROJECT_ROOT=$(echo "$CONTEXT" | jq -r '.project_root')
IS_RESUME=$(echo "$CONTEXT" | jq -r '.is_resume')

cd "$PROJECT_ROOT" 2>/dev/null || exit 0

# Skip reminders on fresh sessions (no work done yet)
if [ "$IS_RESUME" = "false" ]; then
  exit 0
fi

DIRTY=$(git status --porcelain 2>/dev/null | wc -l)
UNPUSHED=$(git rev-list @{upstream}..HEAD 2>/dev/null | wc -l)

if [ "$DIRTY" -gt 0 ] || [ "$UNPUSHED" -gt 0 ]; then
  echo "REMINDER: Before stopping, ensure your work is properly saved:"
  [ "$DIRTY" -gt 0 ] && echo "  - $DIRTY uncommitted file(s) — commit your changes"
  [ "$UNPUSHED" -gt 0 ] && echo "  - $UNPUSHED unpushed commit(s) — push and create a PR"
fi
```

## Full example

```toml
[pre_run]
enabled = true
command = "echo pre-run: {session_id} {tool}"
timeout_secs = 30

[post_run]
enabled = true
command = "echo post-run: {session_id} exit={exit_code}"
timeout_secs = 30

[session_complete]
enabled = true
command = "cd {sessions_root} && git add {session_id}/ && git commit -m 'session {session_id} complete' -q --allow-empty"
timeout_secs = 30

[todo_create]
enabled = true
command = "echo created {plan_id} at {plan_dir}"
timeout_secs = 30

[todo_save]
enabled = true
command = "echo saved {plan_id} v{version}: {message}"
timeout_secs = 30

[[prompt_guard]]
name = "branch-protection"
command = "/path/to/guard-branch.sh"
timeout_secs = 5

[[prompt_guard]]
name = "commit-reminder"
command = "/path/to/remind-commit.sh"
timeout_secs = 10
```
