//! Hook event definitions and metadata.

/// Hook events that trigger hook execution.
///
/// Currently wired trigger points:
/// - `PostRun` — fired after every tool execution in `pipeline::execute_with_session`
/// - `SessionComplete` — fired after session save in `pipeline::execute_with_session`
///
/// Future trigger points (defined but not yet wired):
/// - `PreRun` — will be wired before tool spawn in pipeline
/// - `TodoCreate` — will be wired in `csa-todo` crate on plan creation
/// - `TodoSave` — will be wired in `csa-todo` crate on plan save/update
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    /// After a session execution completes (success or failure).
    /// Triggered in `pipeline::execute_with_session` after session save.
    SessionComplete,
    /// After a new TODO plan is created.
    /// Not yet wired — will integrate with `csa-todo` crate.
    TodoCreate,
    /// After a TODO plan is saved/updated.
    /// Not yet wired — will integrate with `csa-todo` crate.
    TodoSave,
    /// Before a tool execution starts.
    /// Not yet wired — will integrate with pipeline pre-spawn.
    PreRun,
    /// After a tool execution finishes.
    /// Triggered in `pipeline::execute_with_session` after session save.
    PostRun,
}

impl HookEvent {
    /// Returns the TOML configuration key for this event.
    ///
    /// This key is used to look up hook configuration in `hooks.toml`:
    /// ```toml
    /// [session_complete]
    /// enabled = true
    /// command = "..."
    /// ```
    pub fn as_config_key(&self) -> &str {
        match self {
            HookEvent::SessionComplete => "session_complete",
            HookEvent::TodoCreate => "todo_create",
            HookEvent::TodoSave => "todo_save",
            HookEvent::PreRun => "pre_run",
            HookEvent::PostRun => "post_run",
        }
    }

    /// Returns the built-in default command template for this event.
    ///
    /// Built-in commands use template variables wrapped in `{braces}` that are
    /// substituted at runtime with actual values.
    ///
    /// Returns `None` for events that have no built-in default (PreRun, PostRun).
    pub fn builtin_command(&self) -> Option<&str> {
        match self {
            HookEvent::SessionComplete => Some(
                "cd {sessions_root} && git add {session_id}/ && git commit -m 'session {session_id} complete' -q --allow-empty",
            ),
            HookEvent::TodoCreate => Some(
                "cd {todo_root} && git add {plan_dir}/ && git commit -m 'v1: {plan_id} initial' -q",
            ),
            HookEvent::TodoSave => Some(
                "cd {todo_root} && git add {plan_dir}/ && git commit -m 'v{version}: {message}' -q",
            ),
            HookEvent::PreRun | HookEvent::PostRun => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_as_config_key() {
        assert_eq!(
            HookEvent::SessionComplete.as_config_key(),
            "session_complete"
        );
        assert_eq!(HookEvent::TodoCreate.as_config_key(), "todo_create");
        assert_eq!(HookEvent::TodoSave.as_config_key(), "todo_save");
        assert_eq!(HookEvent::PreRun.as_config_key(), "pre_run");
        assert_eq!(HookEvent::PostRun.as_config_key(), "post_run");
    }

    #[test]
    fn test_builtin_command() {
        // Events with built-in commands
        assert!(HookEvent::SessionComplete.builtin_command().is_some());
        assert!(HookEvent::TodoCreate.builtin_command().is_some());
        assert!(HookEvent::TodoSave.builtin_command().is_some());

        // Events without built-in commands
        assert!(HookEvent::PreRun.builtin_command().is_none());
        assert!(HookEvent::PostRun.builtin_command().is_none());
    }

    #[test]
    fn test_builtin_command_content() {
        let cmd = HookEvent::SessionComplete.builtin_command().unwrap();
        assert!(cmd.contains("{session_id}"));
        assert!(cmd.contains("git commit"));

        let cmd = HookEvent::TodoCreate.builtin_command().unwrap();
        assert!(cmd.contains("{plan_id}"));
        assert!(cmd.contains("v1:"));

        let cmd = HookEvent::TodoSave.builtin_command().unwrap();
        assert!(cmd.contains("{version}"));
        assert!(cmd.contains("{message}"));
    }
}
