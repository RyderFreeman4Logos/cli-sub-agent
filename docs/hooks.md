# Hooks

CSA supports lifecycle hooks and prompt guards configured in
`hooks.toml`. Hooks fire at specific points during execution;
prompt guards inject context into tool prompts.

## Configuration File

Hooks are configured in `.csa/hooks.toml` (project-level) or
`~/.config/cli-sub-agent/hooks.toml` (global). Project-level
hooks take precedence.

## Lifecycle Hooks

### `pre_run`

Fires before a tool execution starts in `csa run`.

**Available variables:**

| Variable | Description |
|----------|-------------|
| `{session_id}` | Session ULID |
| `{session_dir}` | Absolute path to session directory |
| `{sessions_root}` | Parent directory containing all project sessions |
| `{tool}` | Tool name (`codex`, `claude-code`, etc.) |

```toml
[pre_run]
enabled = true
command = "echo pre-run: session={session_id} tool={tool}"
timeout_secs = 30
```

### `post_run`

Fires after tool execution completes and session state has been saved.

**Additional variables (beyond `pre_run`):**

| Variable | Description |
|----------|-------------|
| `{exit_code}` | Tool process exit code |

```toml
[post_run]
enabled = true
command = "echo post-run: session={session_id} exit={exit_code}"
timeout_secs = 30
```

### `session_complete`

Fires after `post_run`, at the end of `csa run` execution.

Has the same variables as `post_run`. Default command (when not
configured) commits session state to git:

```toml
[session_complete]
enabled = true
command = "cd {sessions_root} && git add {session_id}/ && git commit -m 'session {session_id} complete' -q --allow-empty"
timeout_secs = 30
```

### `todo_create`

Fires after `csa todo create` commits a new plan.

| Variable | Description |
|----------|-------------|
| `{plan_id}` | TODO plan ULID/timestamp |
| `{plan_dir}` | Absolute path to the plan directory |
| `{todo_root}` | Absolute path to TODO repository root |

```toml
[todo_create]
enabled = true
command = "echo created plan={plan_id}"
timeout_secs = 30
```

### `todo_save`

Fires after `csa todo save` commits updated plan content.

**Additional variables (beyond `todo_create`):**

| Variable | Description |
|----------|-------------|
| `{version}` | Number of saved versions after this save |
| `{message}` | Commit message used |

```toml
[todo_save]
enabled = true
command = "echo saved plan={plan_id} v{version}: {message}"
timeout_secs = 30
```

## Template Variables

Template variables use `{name}` syntax. CSA shell-escapes all substituted
values for safety. Unknown placeholders are left unchanged.

## Prompt Guards

Prompt guards are user-configurable shell scripts that inject text into
the tool's prompt before execution. Unlike lifecycle hooks (fire-and-forget),
prompt guards **capture stdout** and append it to the prompt as
`<prompt-guard>` XML blocks.

This enables "reverse prompt injection" -- reminding tools (including those
without native hook systems like codex, opencode, gemini-cli) to follow
project rules such as branch protection and timely commits.

### How It Works

1. CSA loads `[[prompt_guard]]` entries from `hooks.toml`
2. Before tool execution, runs each guard script sequentially
3. Each script receives a JSON context on **stdin**
4. Non-empty stdout is wrapped in `<prompt-guard name="...">` XML
5. Non-zero exit or timeout = warn + skip (never blocks execution)

### Guard Context (stdin JSON)

```json
{
  "project_root": "/path/to/project",
  "session_id": "01ABCDEF...",
  "tool": "codex",
  "is_resume": false,
  "cwd": "/path/to/project"
}
```

### Script Protocol

| Aspect | Behavior |
|--------|----------|
| **stdin** | JSON `GuardContext` |
| **stdout** | Injection text (empty = no injection) |
| **stderr** | Ignored |
| **exit 0** | Success -- stdout captured |
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
|-------|------|---------|-------------|
| `name` | string | required | Human-readable name (used in XML tag) |
| `command` | string | required | Shell command (run via `sh -c`) |
| `timeout_secs` | integer | 10 | Max execution time in seconds |

### Output Format

Guard output is injected as XML in the effective prompt:

```xml
<prompt-guard name="branch-protection">
You are on branch main. Do NOT commit directly.
Create a feature branch first: git checkout -b feat/description
</prompt-guard>
```

### Merge Behavior

- Guards execute in array order (first entry runs first)
- Project-level `[[prompt_guard]]` **replaces** global-level entirely
- Multiple guards produce multiple `<prompt-guard>` blocks

### Example: Branch Protection Guard

```bash
#!/bin/sh
# guard-branch.sh
set -e
CONTEXT=$(cat)
PROJECT_ROOT=$(echo "$CONTEXT" | jq -r '.project_root')
cd "$PROJECT_ROOT" 2>/dev/null || exit 0

BRANCH=$(git branch --show-current 2>/dev/null) || exit 0
case "$BRANCH" in
  main|master|dev|develop)
    echo "WARNING: You are on protected branch '$BRANCH'."
    echo "Create a feature branch first."
    ;;
esac
```

### Example: Commit Reminder Guard

```bash
#!/bin/sh
# remind-commit.sh
set -e
CONTEXT=$(cat)
PROJECT_ROOT=$(echo "$CONTEXT" | jq -r '.project_root')
IS_RESUME=$(echo "$CONTEXT" | jq -r '.is_resume')
cd "$PROJECT_ROOT" 2>/dev/null || exit 0

[ "$IS_RESUME" = "false" ] && exit 0

DIRTY=$(git status --porcelain 2>/dev/null | wc -l)
UNPUSHED=$(git rev-list @{upstream}..HEAD 2>/dev/null | wc -l)

if [ "$DIRTY" -gt 0 ] || [ "$UNPUSHED" -gt 0 ]; then
  echo "REMINDER: Save your work before stopping:"
  [ "$DIRTY" -gt 0 ] && echo "  - $DIRTY uncommitted file(s)"
  [ "$UNPUSHED" -gt 0 ] && echo "  - $UNPUSHED unpushed commit(s)"
fi
```

## Full Example

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
command = "echo created {plan_id}"
timeout_secs = 30

[todo_save]
enabled = true
command = "echo saved {plan_id} v{version}"
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

## Related

- [Configuration](configuration.md) -- config file locations
- [Skills & Patterns](skills-patterns.md) -- prompt guards complement skills
- [Commands](commands.md) -- `csa run` execution lifecycle
