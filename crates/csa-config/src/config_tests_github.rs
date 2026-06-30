use crate::config::CURRENT_SCHEMA_VERSION;
use crate::{ProjectConfig, ProjectMeta};
use std::collections::HashMap;
use tempfile::tempdir;

struct EnvVarGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set<K: AsRef<std::ffi::OsStr>, V: AsRef<std::ffi::OsStr>>(key: K, value: V) -> Self {
        let key_string = key.as_ref().to_string_lossy().to_string();
        let key_static: &'static str = Box::leak(key_string.into_boxed_str());
        let original = std::env::var_os(key_static);
        // SAFETY: tests set process env in a controlled manner and restore it via Drop.
        unsafe { std::env::set_var(key_static, value) };
        Self {
            key: key_static,
            original,
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.original {
            // SAFETY: tests restore the previous process env value captured in `set`.
            unsafe { std::env::set_var(self.key, value) };
        } else {
            // SAFETY: tests remove the process env var only when it was absent before.
            unsafe { std::env::remove_var(self.key) };
        }
    }
}

#[test]
fn test_resolve_github_config_dir_reports_project_parse_error() {
    let dir = tempdir().unwrap();
    let user_path = dir.path().join("user.toml");
    let project_path = dir.path().join("project").join(".csa").join("config.toml");
    std::fs::create_dir_all(project_path.parent().expect("project config parent")).unwrap();
    std::fs::write(
        &user_path,
        r#"
[github]
config_dir = "/tmp/user-gh"
"#,
    )
    .unwrap();
    std::fs::write(
        &project_path,
        r#"
[github
config_dir = "/tmp/project-gh"
"#,
    )
    .unwrap();

    let err = ProjectConfig::resolve_github_config_dir_with_paths(Some(&user_path), &project_path)
        .unwrap_err();
    let message = format!("{err:#}");

    assert!(message.contains("Failed to parse project config"));
    assert!(message.contains(&project_path.display().to_string()));
}

#[test]
fn test_resolved_github_config_dir_treats_empty_value_as_unset() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", &home);
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", dir.path().join("xdg-config"));
    let config: ProjectConfig = toml::from_str(
        r#"
schema_version = 1

[project]
name = "test-project"

[github]
config_dir = "   "
"#,
    )
    .unwrap();

    assert_eq!(
        config.resolved_github_config_dir(),
        Some(home.join(".config/gh-aider").to_string_lossy().into_owned())
    );
}

#[test]
fn test_resolved_github_config_dir_preserves_trimmed_override() {
    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: Default::default(),
        acp: Default::default(),
        tools: Default::default(),
        review: None,
        debate: None,
        tiers: Default::default(),
        tier_mapping: Default::default(),
        aliases: Default::default(),
        tool_aliases: Default::default(),
        preferences: None,
        github: Some(crate::GithubConfig {
            config_dir: Some("  /tmp/project-gh  ".to_string()),
        }),
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    assert_eq!(
        config.resolved_github_config_dir(),
        Some("/tmp/project-gh".to_string())
    );
}
