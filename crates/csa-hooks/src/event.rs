//! Hook event definitions and metadata.

/// Hook events that trigger hook execution.
///
/// All trigger points are wired:
/// - `PreRun` — fired before tool spawn in `pipeline::execute_with_session_and_meta`
/// - `PostRun` — fired after every tool execution in `pipeline::execute_with_session_and_meta`
/// - `PostEdit` — fired after PostRun when `.rs` files are in changed_paths (observational clippy check)
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
    /// After PostRun when `.rs` files are among changed paths.
    /// Observational: runs a quick clippy check on changed crates.
    /// Triggered in `pipeline_post_exec::process_execution_result`.
    PostEdit,
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
            HookEvent::PostEdit => "post_edit",
        }
    }

    /// Returns whether this event is gatekeeping (controls pipeline flow).
    ///
    /// Gatekeeping events produce a `Result` that the pipeline inspects to
    /// decide whether to continue or abort. Their hook outcome is part of
    /// the control flow contract.
    ///
    /// Observational events are pure notifications — hook failures are logged
    /// but never abort the pipeline. Callers may fire them asynchronously.
    ///
    /// Classification:
    /// - **Gatekeeping**: `PreRun` (can block tool spawn), `SessionComplete`
    ///   (can block session save acknowledgement).
    /// - **Observational**: `PostRun`, `TodoCreate`, `TodoSave` — purely
    ///   informational, no control flow impact.
    pub fn is_gatekeeping(&self) -> bool {
        matches!(self, HookEvent::PreRun | HookEvent::SessionComplete)
    }

    /// Returns the default timeout in seconds for this event.
    ///
    /// Most events use 30s. `PostEdit` uses 120s because `cargo clippy`
    /// on a workspace can take longer than a simple git commit.
    pub fn default_timeout_secs(&self) -> u64 {
        match self {
            HookEvent::PostEdit => 120,
            _ => 30,
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
            HookEvent::PostEdit => {
                Some("cargo clippy {!CHANGED_CRATES_FLAGS} --message-format=short 2>&1 | head -30")
            }
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
        assert_eq!(HookEvent::PostEdit.as_config_key(), "post_edit");
    }

    #[test]
    fn test_builtin_command() {
        // Events with built-in commands
        assert!(HookEvent::SessionComplete.builtin_command().is_some());

        assert!(HookEvent::PostEdit.builtin_command().is_some());

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
            HookEvent::PostEdit,
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
        // Ensure we covered all 6 variants
        assert_eq!(seen_keys.len(), 6, "Expected 6 unique config keys");
    }

    #[test]
    fn test_is_gatekeeping_pre_run() {
        assert!(HookEvent::PreRun.is_gatekeeping());
    }

    #[test]
    fn test_is_gatekeeping_session_complete() {
        assert!(HookEvent::SessionComplete.is_gatekeeping());
    }

    #[test]
    fn test_is_gatekeeping_post_run_false() {
        assert!(!HookEvent::PostRun.is_gatekeeping());
    }

    #[test]
    fn test_is_gatekeeping_todo_create_false() {
        assert!(!HookEvent::TodoCreate.is_gatekeeping());
    }

    #[test]
    fn test_is_gatekeeping_todo_save_false() {
        assert!(!HookEvent::TodoSave.is_gatekeeping());
    }

    #[test]
    fn test_is_gatekeeping_post_edit_false() {
        assert!(!HookEvent::PostEdit.is_gatekeeping());
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
            HookEvent::PostEdit,
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
        let events_with_builtins = [HookEvent::SessionComplete, HookEvent::PostEdit];

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

        let c = a; // Copy
        assert_eq!(a, c);

        // Hash: can be used as HashMap key
        let mut map = std::collections::HashMap::new();
        map.insert(a, "value");
        assert_eq!(map.get(&b), Some(&"value"));
    }
}
