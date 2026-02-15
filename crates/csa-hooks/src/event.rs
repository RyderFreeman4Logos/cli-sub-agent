//! Hook event definitions and metadata.

/// Hook events that trigger hook execution.
///
/// All trigger points are wired:
/// - `PreRun` — fired before tool spawn in `pipeline::execute_with_session_and_meta`
/// - `PostRun` — fired after every tool execution in `pipeline::execute_with_session_and_meta`
/// - `SessionComplete` — fired after session save in `pipeline::execute_with_session_and_meta`
/// - `TodoCreate` — fired after plan creation + git commit in `todo_cmd::handle_create` (no builtin; git already committed)
/// - `TodoSave` — fired after plan save + git commit in `todo_cmd::handle_save` (no builtin; git already committed)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    /// After a session execution completes (success or failure).
    /// Triggered in `pipeline::execute_with_session` after session save.
    SessionComplete,
    /// After a new TODO plan is created.
    /// Triggered in `todo_cmd::handle_create` after git commit.
    TodoCreate,
    /// After a TODO plan is saved/updated.
    /// Triggered in `todo_cmd::handle_save` after git commit.
    TodoSave,
    /// Before a tool execution starts.
    /// Triggered in `pipeline::execute_with_session_and_meta` before `execute_with_transport`.
    PreRun,
    /// After a tool execution finishes.
    /// Triggered in `pipeline::execute_with_session_and_meta` after session save.
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
    /// Returns `None` for events that have no built-in default.
    ///
    /// `TodoCreate` and `TodoSave` have no builtins because `csa_todo::git::save()`
    /// already commits changes before hooks fire; a default `git commit` would fail
    /// on a clean index. Users can still configure custom commands via `hooks.toml`.
    pub fn builtin_command(&self) -> Option<&str> {
        match self {
            HookEvent::SessionComplete => Some(
                "cd {sessions_root} && git add {session_id}/ && git commit -m 'session {session_id} complete' -q --allow-empty",
            ),
            HookEvent::TodoCreate
            | HookEvent::TodoSave
            | HookEvent::PreRun
            | HookEvent::PostRun => None,
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

        // Events without built-in commands (TodoCreate/TodoSave have no builtins
        // because git::save() already commits before hooks fire)
        assert!(HookEvent::TodoCreate.builtin_command().is_none());
        assert!(HookEvent::TodoSave.builtin_command().is_none());
        assert!(HookEvent::PreRun.builtin_command().is_none());
        assert!(HookEvent::PostRun.builtin_command().is_none());
    }

    #[test]
    fn test_builtin_command_content() {
        let cmd = HookEvent::SessionComplete.builtin_command().unwrap();
        assert!(cmd.contains("{session_id}"));
        assert!(cmd.contains("git commit"));

        // TodoCreate and TodoSave have no builtins (covered by test_builtin_command)
    }

    /// Exhaustive test: all variants return distinct, non-empty config keys.
    #[test]
    fn test_all_config_keys_unique_and_nonempty() {
        let all_events = [
            HookEvent::SessionComplete,
            HookEvent::TodoCreate,
            HookEvent::TodoSave,
            HookEvent::PreRun,
            HookEvent::PostRun,
        ];

        let mut seen_keys = std::collections::HashSet::new();
        for event in &all_events {
            let key = event.as_config_key();
            assert!(!key.is_empty(), "{event:?} has empty config key");
            assert!(
                seen_keys.insert(key),
                "Duplicate config key: {key} (from {event:?})"
            );
        }
        // Ensure we covered all 5 variants
        assert_eq!(seen_keys.len(), 5, "Expected 5 unique config keys");
    }

    /// Verify config keys match the expected snake_case convention.
    #[test]
    fn test_config_keys_are_snake_case() {
        let all_events = [
            HookEvent::SessionComplete,
            HookEvent::TodoCreate,
            HookEvent::TodoSave,
            HookEvent::PreRun,
            HookEvent::PostRun,
        ];

        for event in &all_events {
            let key = event.as_config_key();
            assert!(
                key.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "Config key {key:?} for {event:?} is not snake_case"
            );
        }
    }

    /// Boundary: builtin_command for events WITH builtins always contains
    /// at least one template variable placeholder.
    #[test]
    fn test_builtin_commands_contain_template_vars() {
        let events_with_builtins = [HookEvent::SessionComplete];

        for event in &events_with_builtins {
            let cmd = event
                .builtin_command()
                .unwrap_or_else(|| panic!("{event:?} should have a builtin command"));
            assert!(
                cmd.contains('{') && cmd.contains('}'),
                "Builtin command for {event:?} should contain template variables, got: {cmd}"
            );
        }
    }

    /// Verify SessionComplete builtin contains all required variables.
    #[test]
    fn test_session_complete_builtin_has_required_vars() {
        let cmd = HookEvent::SessionComplete.builtin_command().unwrap();
        assert!(
            cmd.contains("{sessions_root}"),
            "SessionComplete builtin must reference sessions_root"
        );
        assert!(
            cmd.contains("{session_id}"),
            "SessionComplete builtin must reference session_id"
        );
    }

    /// Boundary: HookEvent derives Copy, Clone, PartialEq, Eq, Hash correctly.
    #[test]
    fn test_hook_event_traits() {
        let a = HookEvent::PreRun;
        let b = a; // Copy
        assert_eq!(a, b); // PartialEq + Eq

        let c = a.clone(); // Clone
        assert_eq!(a, c);

        // Hash: can be used as HashMap key
        let mut map = std::collections::HashMap::new();
        map.insert(a, "value");
        assert_eq!(map.get(&b), Some(&"value"));
    }
}
