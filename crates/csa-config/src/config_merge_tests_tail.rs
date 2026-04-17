use super::*;
use tempfile::tempdir;

// ── Task 2: HooksSection tests ──────────────────────────────────────

#[test]
fn test_hooks_section_parses_from_toml() {
    let toml_str = r#"
schema_version = 1
[hooks]
pre_run = "cargo fmt --all"
post_run = "cargo clippy"
timeout_secs = 120
"#;
    let config: ProjectConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.hooks.pre_run.as_deref(), Some("cargo fmt --all"));
    assert_eq!(config.hooks.post_run.as_deref(), Some("cargo clippy"));
    assert_eq!(config.hooks.timeout_secs, 120);
}

#[test]
fn test_hooks_section_default_is_empty() {
    let config: ProjectConfig = toml::from_str("schema_version = 1\n").unwrap();
    assert!(config.hooks.is_default());
    assert!(config.hooks.pre_run.is_none());
    assert!(config.hooks.post_run.is_none());
    assert_eq!(config.hooks.timeout_secs, 60);
}

#[test]
fn test_hooks_section_is_default_returns_false_with_pre_run() {
    let mut hooks = crate::config::HooksSection::default();
    assert!(hooks.is_default());
    hooks.pre_run = Some("echo hi".to_string());
    assert!(!hooks.is_default());
}

#[test]
fn test_hooks_section_is_default_returns_false_with_post_run() {
    let hooks = crate::config::HooksSection {
        post_run: Some("echo bye".to_string()),
        ..Default::default()
    };
    assert!(!hooks.is_default());
}

#[test]
fn test_hooks_section_is_default_returns_false_with_custom_timeout() {
    let hooks = crate::config::HooksSection {
        timeout_secs: 120,
        ..Default::default()
    };
    assert!(!hooks.is_default());
}

#[test]
fn test_hooks_section_default_serialization_omits_section() {
    let config: ProjectConfig = toml::from_str("schema_version = 1\n").unwrap();
    let output = toml::to_string(&config).unwrap();
    // Default hooks should not appear in serialized output (skip_serializing_if)
    assert!(
        !output.contains("[hooks]"),
        "Default hooks section should be omitted from TOML output"
    );
}

#[test]
fn test_hooks_section_non_default_serialized() {
    let toml_str = r#"
schema_version = 1
[hooks]
pre_run = "cargo fmt"
"#;
    let config: ProjectConfig = toml::from_str(toml_str).unwrap();
    let output = toml::to_string(&config).unwrap();
    assert!(
        output.contains("[hooks]"),
        "Non-default hooks should appear in TOML output"
    );
    assert!(output.contains("pre_run"));
}

#[test]
fn test_hooks_section_project_overrides_user_during_merge() {
    let tmp = tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[hooks]
pre_run = "echo user-pre"
post_run = "echo user-post"
timeout_secs = 30
"#,
    )
    .unwrap();

    std::fs::write(
        &project_path,
        r#"
schema_version = 1
[hooks]
pre_run = "echo project-pre"
"#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .expect("Should load merged config");

    // Project pre_run wins
    assert_eq!(config.hooks.pre_run.as_deref(), Some("echo project-pre"));
    // post_run inherited from user (deep merge)
    assert_eq!(config.hooks.post_run.as_deref(), Some("echo user-post"));
    // timeout_secs inherited from user (not overridden by project)
    assert_eq!(config.hooks.timeout_secs, 30);
}

#[test]
fn test_hooks_section_partial_config_with_only_timeout() {
    let toml_str = r#"
schema_version = 1
[hooks]
timeout_secs = 90
"#;
    let config: ProjectConfig = toml::from_str(toml_str).unwrap();
    assert!(config.hooks.pre_run.is_none());
    assert!(config.hooks.post_run.is_none());
    assert_eq!(config.hooks.timeout_secs, 90);
    // Non-default timeout means is_default() should be false
    assert!(!config.hooks.is_default());
}

// ── Task 5: ExecutionConfig tests ───────────────────────────────────

#[test]
fn test_execution_config_parses_from_toml() {
    let toml_str = r#"
schema_version = 1
[execution]
min_timeout_seconds = 2400
"#;
    let config: ProjectConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.execution.min_timeout_seconds, 2400);
}

#[test]
fn test_execution_config_default() {
    let config: ProjectConfig = toml::from_str("schema_version = 1\n").unwrap();
    assert!(config.execution.is_default());
    assert_eq!(config.execution.min_timeout_seconds, 1800);
    assert_eq!(config.execution.acp_crash_max_attempts, 2);
}

#[test]
fn test_execution_config_is_default() {
    let exec = crate::config::ExecutionConfig::default();
    assert!(exec.is_default());
    assert_eq!(exec.min_timeout_seconds, 1800);
    assert_eq!(exec.acp_crash_max_attempts, 2);
}

#[test]
fn test_execution_config_is_not_default_with_custom_value() {
    let exec = crate::config::ExecutionConfig {
        min_timeout_seconds: 2400,
        acp_crash_max_attempts: 2,
        auto_weave_upgrade: false,
    };
    assert!(!exec.is_default());
}

#[test]
fn test_execution_config_resolved_acp_crash_max_attempts_clamps() {
    let mut exec = crate::config::ExecutionConfig {
        acp_crash_max_attempts: 0,
        ..Default::default()
    };
    assert_eq!(exec.resolved_acp_crash_max_attempts(), 1);

    exec.acp_crash_max_attempts = 2;
    assert_eq!(exec.resolved_acp_crash_max_attempts(), 2);

    exec.acp_crash_max_attempts = 5;
    assert_eq!(exec.resolved_acp_crash_max_attempts(), 5);

    exec.acp_crash_max_attempts = 10;
    assert_eq!(exec.resolved_acp_crash_max_attempts(), 5);
}

#[test]
fn test_execution_config_default_serialization_omits_section() {
    let config: ProjectConfig = toml::from_str("schema_version = 1\n").unwrap();
    let output = toml::to_string(&config).unwrap();
    assert!(
        !output.contains("[execution]"),
        "Default execution section should be omitted from TOML output"
    );
}

#[test]
fn test_execution_config_non_default_serialized() {
    let toml_str = r#"
schema_version = 1
[execution]
min_timeout_seconds = 2400
"#;
    let config: ProjectConfig = toml::from_str(toml_str).unwrap();
    let output = toml::to_string(&config).unwrap();
    assert!(
        output.contains("[execution]"),
        "Non-default execution should appear in TOML output"
    );
    assert!(output.contains("min_timeout_seconds = 2400"));
}

#[test]
fn test_execution_config_project_overrides_user_during_merge() {
    let tmp = tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[execution]
min_timeout_seconds = 1800
"#,
    )
    .unwrap();

    std::fs::write(
        &project_path,
        r#"
schema_version = 1
[execution]
min_timeout_seconds = 2400
"#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .expect("Should load merged config");

    // Project overrides user
    assert_eq!(config.execution.min_timeout_seconds, 2400);
}

#[test]
fn test_execution_config_user_fallback_when_project_omits() {
    let tmp = tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[execution]
min_timeout_seconds = 3600
"#,
    )
    .unwrap();

    std::fs::write(
        &project_path,
        r#"
schema_version = 1
[project]
name = "test"
"#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .expect("Should load merged config");

    // User value inherited when project doesn't set execution
    assert_eq!(config.execution.min_timeout_seconds, 3600);
}
