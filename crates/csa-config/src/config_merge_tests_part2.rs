#[test]
fn test_load_project_overrides_user_tiers() {
    // When both user and project configs exist, project tiers
    // override user tiers (deep merge).
    let tmp = tempfile::tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[tiers.shared-tier]
description = "User version"
models = ["codex/openai/o3/medium"]
"#,
    )
    .unwrap();

    std::fs::write(
        &project_path,
        r#"
schema_version = 1
[tiers.shared-tier]
description = "Project version"
models = ["codex/openai/o3/high"]
"#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .expect("Should load merged config");
    let tier = config.tiers.get("shared-tier").unwrap();
    assert_eq!(tier.description, "Project version");
    assert_eq!(tier.models, vec!["codex/openai/o3/high"]);
}

#[test]
fn test_tool_state_dirs_inherit_from_global_config() {
    let tmp = tempfile::tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[tool_state_dirs]
codex = "/srv/codex-state"
claude = "/srv/claude-state"
"#,
    )
    .unwrap();

    std::fs::write(
        &project_path,
        r#"
schema_version = 1
[project]
name = "state-dir-merge"
"#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .expect("Should load merged config");

    assert_eq!(
        config.tool_state_dirs.get("codex"),
        Some(&PathBuf::from("/srv/codex-state"))
    );
    assert_eq!(
        config.tool_state_dirs.get("claude"),
        Some(&PathBuf::from("/srv/claude-state"))
    );
}

#[test]
fn test_liveness_dead_seconds_priority_project_over_user_over_default() {
    let tmp = tempfile::tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[resources]
liveness_dead_seconds = 700
"#,
    )
    .unwrap();

    std::fs::write(
        &project_path,
        r#"
schema_version = 1
[resources]
liveness_dead_seconds = 120
"#,
    )
    .unwrap();

    let merged = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .unwrap();
    assert_eq!(merged.resources.liveness_dead_seconds, Some(120));

    // user/global fallback
    let user_only =
        ProjectConfig::load_with_paths(Some(&user_path), &tmp.path().join("missing.toml"))
            .unwrap()
            .unwrap();
    assert_eq!(user_only.resources.liveness_dead_seconds, Some(700));

    // built-in default fallback
    let default = ResourcesConfig::default();
    assert_eq!(default.liveness_dead_seconds, Some(600));
}

#[test]
fn test_global_disable_wins_over_project_enable() {
    // Global config disables opencode; project config enables it.
    // After merge, opencode must remain disabled.
    let tmp = tempfile::tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[tools.opencode]
enabled = false
suppress_notify = true
[tools.codex]
enabled = true
suppress_notify = true
"#,
    )
    .unwrap();

    std::fs::write(
        &project_path,
        r#"
schema_version = 1
[tools.opencode]
enabled = true
suppress_notify = true
[tools.codex]
enabled = true
suppress_notify = true
[tiers.tier-1-quick]
description = "Quick"
models = ["opencode/openai/gpt-5/xhigh"]
"#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .expect("Should load merged config");

    // opencode must be disabled (global wins)
    assert!(
        !config.is_tool_enabled("opencode"),
        "globally-disabled tool must remain disabled after merge"
    );
    // codex stays enabled (both agree)
    assert!(config.is_tool_enabled("codex"));
    // opencode must not be auto-selectable
    assert!(
        !config.is_tool_auto_selectable("opencode"),
        "globally-disabled tool must not be auto-selectable"
    );
}

#[test]
fn test_global_disable_wins_tool_only_in_global() {
    // Tool disabled in global but not mentioned in project config at all.
    // After merge, tool should still be disabled.
    let tmp = tempfile::tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[tools.opencode]
enabled = false
suppress_notify = true
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

    assert!(
        !config.is_tool_enabled("opencode"),
        "tool disabled only in global must remain disabled"
    );
}

#[test]
fn test_global_enable_can_be_overridden_by_project_disable() {
    // Global enables a tool, project disables it. Project wins (standard merge).
    let tmp = tempfile::tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[tools.codex]
enabled = true
suppress_notify = true
"#,
    )
    .unwrap();

    std::fs::write(
        &project_path,
        r#"
schema_version = 1
[tools.codex]
enabled = false
suppress_notify = true
"#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .expect("Should load merged config");

    // Project disabling an enabled tool is fine (standard overlay)
    assert!(
        !config.is_tool_enabled("codex"),
        "project can still disable a globally-enabled tool"
    );
}

#[test]
fn test_global_gate_command_ignored_during_merge() {
    // gate_command is project-only. If global sets it, it should be stripped
    // and the merged config should not inherit it.
    let tmp = tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[review]
tool = "auto"
gate_command = "make lint"
gate_timeout_secs = 999
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

    // gate_command from global must be stripped (project-only field)
    assert!(
        config
            .review
            .as_ref()
            .is_none_or(|r| r.gate_command.is_none()),
        "global gate_command must not be inherited by project config"
    );
    // gate_timeout_secs from global must be stripped, falling back to default 250
    assert!(
        config
            .review
            .as_ref()
            .is_none_or(|r| r.gate_timeout_secs == 250),
        "global gate_timeout_secs must not be inherited; should be default 250"
    );
}

#[test]
fn test_project_gate_command_preserved_during_merge() {
    // When the project sets gate_command, it should be preserved.
    let tmp = tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[review]
tool = "auto"
"#,
    )
    .unwrap();

    std::fs::write(
        &project_path,
        r#"
schema_version = 1
[review]
gate_command = "just pre-commit"
gate_timeout_secs = 600
"#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .expect("Should load merged config");

    assert_eq!(
        config
            .review
            .as_ref()
            .and_then(|r| r.gate_command.as_deref()),
        Some("just pre-commit"),
        "project gate_command must be preserved"
    );
    assert_eq!(
        config.review.as_ref().map(|r| r.gate_timeout_secs),
        Some(600),
        "project gate_timeout_secs must be preserved"
    );
}

#[test]
fn test_project_gate_command_overrides_global_gate_command() {
    // When both global and project set gate_command, project wins
    // AND global value is stripped (not merely overridden).
    let tmp = tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[review]
tool = "codex"
gate_command = "make lint"
gate_timeout_secs = 999
"#,
    )
    .unwrap();

    std::fs::write(
        &project_path,
        r#"
schema_version = 1
[review]
gate_command = "just pre-commit"
gate_timeout_secs = 120
"#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .expect("Should load merged config");

    // Project gate_command wins
    assert_eq!(
        config
            .review
            .as_ref()
            .and_then(|r| r.gate_command.as_deref()),
        Some("just pre-commit"),
    );
    assert_eq!(
        config.review.as_ref().map(|r| r.gate_timeout_secs),
        Some(120),
    );
    // tool should still be inherited from global (since project didn't set it)
    assert_eq!(
        config.review.as_ref().map(|r| r.tool.to_string()),
        Some("codex".to_string()),
    );
}

#[test]
fn test_gate_mode_still_inherits_from_global() {
    // gate_mode is NOT project-only; normal merge applies.
    let tmp = tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[review]
gate_mode = "full"
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

    // gate_mode from global should be inherited (not project-only)
    assert_eq!(
        config.review.as_ref().map(|r| r.gate_mode.clone()),
        Some(crate::global::GateMode::Full),
        "gate_mode should be inherited from global config"
    );
}
