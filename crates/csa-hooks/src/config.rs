//! Hook configuration loading with 4-tier priority.

use crate::event::HookEvent;
use crate::guard::PromptGuardEntry;
use crate::guard::builtin_prompt_guards;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Configuration for a single hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// Whether this hook is enabled (default: true for built-in events)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Shell command template. If None, uses the built-in default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Timeout in seconds (default: 30)
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_true() -> bool {
    true
}
fn default_timeout() -> u64 {
    30
}

/// All hooks configuration, keyed by event name.
///
/// The `prompt_guard` field is an independent typed array that does NOT go
/// through the `flatten` HashMap. This allows `[[prompt_guard]]` TOML arrays
/// to be deserialized as structured entries alongside the flat hook map.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    /// Whether to prepend built-in prompt guards before user-defined guards.
    /// `None` = not specified (inherit from lower-priority layer; defaults to `true`).
    /// `Some(false)` = explicitly disable builtins. `Some(true)` = explicitly enable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin_guards: Option<bool>,

    /// User-configurable prompt guard scripts. Each entry is a shell script
    /// that receives JSON context on stdin and outputs injection text on stdout.
    #[serde(default)]
    pub prompt_guard: Vec<PromptGuardEntry>,

    #[serde(flatten)]
    pub hooks: HashMap<String, HookConfig>,
}

impl HooksConfig {
    /// Load from a TOML file, returning empty config on error.
    fn load_from_file(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }

        match fs::read_to_string(path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(config) => config,
                Err(e) => {
                    tracing::warn!("Failed to parse hooks config at {}: {}", path.display(), e);
                    Self::default()
                }
            },
            Err(e) => {
                tracing::warn!("Failed to read hooks config at {}: {}", path.display(), e);
                Self::default()
            }
        }
    }

    /// Merge another config into self, with other taking priority.
    ///
    /// For hooks: higher-priority entries override by key.
    /// For prompt_guard: higher-priority entries replace the entire array
    /// (non-empty array wins; empty array means "no override from this layer").
    fn merge_with(&mut self, other: Self) {
        // builtin_guards: only override when explicitly set (Some); None = no opinion
        if other.builtin_guards.is_some() {
            self.builtin_guards = other.builtin_guards;
        }
        for (key, value) in other.hooks {
            self.hooks.insert(key, value);
        }
        // prompt_guard: non-empty higher-priority array replaces lower
        if !other.prompt_guard.is_empty() {
            self.prompt_guard = other.prompt_guard;
        }
    }

    /// Get configuration for a specific event, falling back to built-in defaults.
    pub fn get_for_event(&self, event: HookEvent) -> HookConfig {
        let key = event.as_config_key();
        if let Some(config) = self.hooks.get(key) {
            config.clone()
        } else if event.builtin_command().is_some() {
            // Use built-in default for events that have one
            HookConfig {
                enabled: true,
                command: None, // Will be resolved from builtin_command()
                timeout_secs: 30,
            }
        } else {
            // Events without built-in: disabled by default
            HookConfig {
                enabled: false,
                command: None,
                timeout_secs: 30,
            }
        }
    }
}

/// Load hooks config with 4-tier priority:
/// 1. runtime_overrides (CLI params) — highest
/// 2. project config (~/.local/state/csa/{project}/hooks.toml)
/// 3. global config (~/.config/cli-sub-agent/hooks.toml)
/// 4. built-in defaults — lowest
///
/// For each hook event key, the first non-None source wins.
pub fn load_hooks_config(
    project_hooks_path: Option<&Path>,
    global_hooks_path: Option<&Path>,
    runtime_overrides: Option<&HashMap<String, HookConfig>>,
) -> HooksConfig {
    // Start with built-in defaults (empty map, resolved on-demand via get_for_event)
    let mut config = HooksConfig::default();

    // Layer 4 (lowest): built-in defaults are implicit in get_for_event()

    // Layer 3: global config
    if let Some(path) = global_hooks_path {
        let global = HooksConfig::load_from_file(path);
        config.merge_with(global);
    }

    // Layer 2: project config
    if let Some(path) = project_hooks_path {
        let project = HooksConfig::load_from_file(path);
        config.merge_with(project);
    }

    // Layer 1 (highest): runtime overrides
    if let Some(overrides) = runtime_overrides {
        let runtime_config = HooksConfig {
            builtin_guards: None, // runtime overrides don't change guard enablement
            prompt_guard: Vec::new(),
            hooks: overrides.clone(),
        };
        config.merge_with(runtime_config);
    }

    // Layer 0 (lowest): built-in prompt guards prepended to user guards.
    // Builtins run first, then user-defined guards. `builtin_guards = false` disables.
    if config.builtin_guards.unwrap_or(true) {
        let mut combined = builtin_prompt_guards();
        combined.extend(config.prompt_guard);
        config.prompt_guard = combined;
    }

    config
}

/// Resolve the global hooks config path
pub fn global_hooks_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("", "", "cli-sub-agent")
        .map(|dirs| dirs.config_dir().join("hooks.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_empty_config() {
        let config = load_hooks_config(None, None, None);
        // Should use built-in defaults
        let session_hook = config.get_for_event(HookEvent::SessionComplete);
        assert!(session_hook.enabled);
        assert!(session_hook.command.is_none()); // Will resolve from builtin
    }

    #[test]
    fn test_load_global_config() {
        let mut global_file = NamedTempFile::new().unwrap();
        writeln!(
            global_file,
            r#"
[session_complete]
enabled = false
"#
        )
        .unwrap();
        global_file.flush().unwrap();

        let config = load_hooks_config(None, Some(global_file.path()), None);
        let session_hook = config.get_for_event(HookEvent::SessionComplete);
        assert!(!session_hook.enabled);
    }

    #[test]
    fn test_priority_merge() {
        let mut global_file = NamedTempFile::new().unwrap();
        writeln!(
            global_file,
            r#"
[session_complete]
enabled = false
"#
        )
        .unwrap();
        global_file.flush().unwrap();

        let mut project_file = NamedTempFile::new().unwrap();
        writeln!(
            project_file,
            r#"
[session_complete]
enabled = true
command = "echo project"
"#
        )
        .unwrap();
        project_file.flush().unwrap();

        // Project should override global
        let config = load_hooks_config(Some(project_file.path()), Some(global_file.path()), None);
        let session_hook = config.get_for_event(HookEvent::SessionComplete);
        assert!(session_hook.enabled);
        assert_eq!(session_hook.command.as_deref(), Some("echo project"));
    }

    #[test]
    fn test_runtime_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "session_complete".to_string(),
            HookConfig {
                enabled: false,
                command: Some("echo runtime".to_string()),
                timeout_secs: 10,
            },
        );

        let mut project_file = NamedTempFile::new().unwrap();
        writeln!(
            project_file,
            r#"
[session_complete]
enabled = true
command = "echo project"
"#
        )
        .unwrap();
        project_file.flush().unwrap();

        // Runtime should override project
        let config = load_hooks_config(Some(project_file.path()), None, Some(&overrides));
        let session_hook = config.get_for_event(HookEvent::SessionComplete);
        assert!(!session_hook.enabled);
        assert_eq!(session_hook.command.as_deref(), Some("echo runtime"));
        assert_eq!(session_hook.timeout_secs, 10);
    }

    #[test]
    fn test_events_without_builtin() {
        let config = load_hooks_config(None, None, None);
        let pre_run_hook = config.get_for_event(HookEvent::PreRun);
        // PreRun has no built-in, should be disabled by default
        assert!(!pre_run_hook.enabled);
    }

    #[test]
    fn test_load_from_tempdir_with_hooks_toml() {
        let dir = tempfile::tempdir().unwrap();
        let hooks_path = dir.path().join("hooks.toml");
        std::fs::write(
            &hooks_path,
            r#"
[pre_run]
enabled = true
command = "echo pre-run"
timeout_secs = 15

[post_run]
enabled = true
command = "echo post-run"
timeout_secs = 20
"#,
        )
        .unwrap();

        let config = load_hooks_config(Some(&hooks_path), None, None);

        let pre_run = config.get_for_event(HookEvent::PreRun);
        assert!(pre_run.enabled);
        assert_eq!(pre_run.command.as_deref(), Some("echo pre-run"));
        assert_eq!(pre_run.timeout_secs, 15);

        let post_run = config.get_for_event(HookEvent::PostRun);
        assert!(post_run.enabled);
        assert_eq!(post_run.command.as_deref(), Some("echo post-run"));
        assert_eq!(post_run.timeout_secs, 20);
    }

    #[test]
    fn test_get_for_event_all_variants() {
        let config = load_hooks_config(None, None, None);

        // Events with built-in commands default to enabled
        let session = config.get_for_event(HookEvent::SessionComplete);
        assert!(session.enabled);
        assert!(session.command.is_none()); // resolved lazily

        // Events without built-in commands default to disabled
        // (TodoCreate/TodoSave have no builtins — git::save() already commits)
        let todo_create = config.get_for_event(HookEvent::TodoCreate);
        assert!(!todo_create.enabled);

        let todo_save = config.get_for_event(HookEvent::TodoSave);
        assert!(!todo_save.enabled);

        let pre_run = config.get_for_event(HookEvent::PreRun);
        assert!(!pre_run.enabled);

        let post_run = config.get_for_event(HookEvent::PostRun);
        assert!(!post_run.enabled);
    }

    #[test]
    fn test_missing_config_file_returns_default() {
        let nonexistent = Path::new("/nonexistent/dir/hooks.toml");
        let config = load_hooks_config(Some(nonexistent), None, None);

        // Should fall back to default (empty hooks map)
        assert!(config.hooks.is_empty());

        // Built-in events should still work via get_for_event defaults
        let session = config.get_for_event(HookEvent::SessionComplete);
        assert!(session.enabled);
    }

    #[test]
    fn test_malformed_toml_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let hooks_path = dir.path().join("hooks.toml");
        std::fs::write(&hooks_path, "this is not valid toml {{{{").unwrap();

        let config = load_hooks_config(Some(&hooks_path), None, None);

        // Should gracefully return default config
        assert!(config.hooks.is_empty());
    }

    #[test]
    fn test_default_timeout_value() {
        let dir = tempfile::tempdir().unwrap();
        let hooks_path = dir.path().join("hooks.toml");
        // Only set enabled and command; timeout_secs should get default 30
        std::fs::write(
            &hooks_path,
            r#"
[pre_run]
enabled = true
command = "echo test"
"#,
        )
        .unwrap();

        let config = load_hooks_config(Some(&hooks_path), None, None);
        let pre_run = config.get_for_event(HookEvent::PreRun);
        assert_eq!(pre_run.timeout_secs, 30, "Default timeout should be 30");
    }

    #[test]
    fn test_merge_global_and_project_partial_overlap() {
        let dir = tempfile::tempdir().unwrap();

        let global_path = dir.path().join("global_hooks.toml");
        std::fs::write(
            &global_path,
            r#"
[pre_run]
enabled = true
command = "echo global-pre"

[post_run]
enabled = true
command = "echo global-post"
"#,
        )
        .unwrap();

        let project_path = dir.path().join("project_hooks.toml");
        std::fs::write(
            &project_path,
            r#"
[pre_run]
enabled = false
command = "echo project-pre"
"#,
        )
        .unwrap();

        let config = load_hooks_config(Some(&project_path), Some(&global_path), None);

        // pre_run: project overrides global
        let pre_run = config.get_for_event(HookEvent::PreRun);
        assert!(!pre_run.enabled);
        assert_eq!(pre_run.command.as_deref(), Some("echo project-pre"));

        // post_run: only in global, not overridden
        let post_run = config.get_for_event(HookEvent::PostRun);
        assert!(post_run.enabled);
        assert_eq!(post_run.command.as_deref(), Some("echo global-post"));
    }

    #[test]
    fn test_empty_hooks_toml_file() {
        let dir = tempfile::tempdir().unwrap();
        let hooks_path = dir.path().join("hooks.toml");
        std::fs::write(&hooks_path, "").unwrap();

        let config = load_hooks_config(Some(&hooks_path), None, None);
        assert!(config.hooks.is_empty());
    }

    #[test]
    fn test_builtin_guards_loaded_by_default() {
        let config = load_hooks_config(None, None, None);
        assert_eq!(config.prompt_guard.len(), 3);
        assert_eq!(config.prompt_guard[0].name, "branch-protection");
        assert_eq!(config.prompt_guard[1].name, "dirty-tree-reminder");
        assert_eq!(config.prompt_guard[2].name, "commit-workflow");
        assert_eq!(config.builtin_guards, None); // None = default to true
    }

    #[test]
    fn test_builtin_guards_prepended_to_user_config() {
        let dir = tempfile::tempdir().unwrap();
        let hooks_path = dir.path().join("hooks.toml");
        std::fs::write(
            &hooks_path,
            r#"
[[prompt_guard]]
name = "custom-guard"
command = "echo custom"
timeout_secs = 10
"#,
        )
        .unwrap();

        let config = load_hooks_config(Some(&hooks_path), None, None);
        // Builtins (3) prepended + user guard (1) = 4 total
        assert_eq!(config.prompt_guard.len(), 4);
        assert_eq!(config.prompt_guard[0].name, "branch-protection");
        assert_eq!(config.prompt_guard[1].name, "dirty-tree-reminder");
        assert_eq!(config.prompt_guard[2].name, "commit-workflow");
        assert_eq!(config.prompt_guard[3].name, "custom-guard");
    }

    #[test]
    fn test_builtin_guards_disabled_with_user_config() {
        let dir = tempfile::tempdir().unwrap();
        let hooks_path = dir.path().join("hooks.toml");
        std::fs::write(
            &hooks_path,
            r#"
builtin_guards = false

[[prompt_guard]]
name = "custom-guard"
command = "echo custom"
timeout_secs = 10
"#,
        )
        .unwrap();

        let config = load_hooks_config(Some(&hooks_path), None, None);
        // Builtins disabled, only user guard remains
        assert_eq!(config.prompt_guard.len(), 1);
        assert_eq!(config.prompt_guard[0].name, "custom-guard");
    }

    #[test]
    fn test_builtin_guards_disabled_explicitly() {
        let dir = tempfile::tempdir().unwrap();
        let hooks_path = dir.path().join("hooks.toml");
        std::fs::write(
            &hooks_path,
            r#"
builtin_guards = false
"#,
        )
        .unwrap();

        let config = load_hooks_config(Some(&hooks_path), None, None);
        assert!(config.prompt_guard.is_empty());
        assert_eq!(config.builtin_guards, Some(false));
    }

    #[test]
    fn test_builtin_guards_global_disable_project_reenable() {
        let dir = tempfile::tempdir().unwrap();

        let global_path = dir.path().join("global_hooks.toml");
        std::fs::write(&global_path, "builtin_guards = false\n").unwrap();

        let project_path = dir.path().join("project_hooks.toml");
        std::fs::write(&project_path, "builtin_guards = true\n").unwrap();

        // Project explicitly re-enables after global disable
        let config = load_hooks_config(Some(&project_path), Some(&global_path), None);
        assert_eq!(config.prompt_guard.len(), 3);
        assert_eq!(config.builtin_guards, Some(true));
    }

    #[test]
    fn test_builtin_guards_global_disable_project_omit_inherits() {
        let dir = tempfile::tempdir().unwrap();

        let global_path = dir.path().join("global_hooks.toml");
        std::fs::write(&global_path, "builtin_guards = false\n").unwrap();

        let project_path = dir.path().join("project_hooks.toml");
        // Omit builtin_guards entirely
        std::fs::write(&project_path, "[pre_run]\nenabled = true\n").unwrap();

        // Project doesn't mention builtin_guards → inherits global false
        let config = load_hooks_config(Some(&project_path), Some(&global_path), None);
        assert!(config.prompt_guard.is_empty());
        assert_eq!(config.builtin_guards, Some(false));
    }
}
