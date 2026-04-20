use super::*;
use tempfile::tempdir;

#[test]
fn test_preflight_ai_config_symlink_check_defaults() {
    let config: ProjectConfig = toml::from_str(
        r#"
schema_version = 2

[project]
name = "test"
"#,
    )
    .expect("parse config");

    assert!(!config.preflight.ai_config_symlink_check.enabled);
    assert!(config.preflight.ai_config_symlink_check.paths.is_none());
    assert!(
        config
            .preflight
            .ai_config_symlink_check
            .treat_broken_symlink_as_error
    );
}

#[test]
fn test_project_preflight_overrides_global_merge() {
    let dir = tempdir().unwrap();
    let user_config = dir.path().join("user.toml");
    let project_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&project_dir).unwrap();
    let project_config = project_dir.join("config.toml");

    std::fs::write(
        &user_config,
        r#"
[preflight.ai_config_symlink_check]
enabled = true
treat_broken_symlink_as_error = true
paths = ["AGENTS.md", "CLAUDE.md"]
"#,
    )
    .unwrap();
    std::fs::write(
        &project_config,
        r#"
[preflight.ai_config_symlink_check]
paths = ["CUSTOM.md"]
treat_broken_symlink_as_error = false
"#,
    )
    .unwrap();

    let merged = ProjectConfig::load_with_paths(Some(&user_config), &project_config)
        .expect("load merged")
        .expect("config present");

    assert!(merged.preflight.ai_config_symlink_check.enabled);
    assert_eq!(
        merged.preflight.ai_config_symlink_check.paths,
        Some(vec!["CUSTOM.md".to_string()])
    );
    assert!(
        !merged
            .preflight
            .ai_config_symlink_check
            .treat_broken_symlink_as_error
    );
}
