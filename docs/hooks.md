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
```
