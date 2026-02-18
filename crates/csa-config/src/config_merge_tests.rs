use super::*;
use tempfile::tempdir;

#[test]
fn test_merge_tiers_deep_merge() {
    let user_toml: toml::Value = toml::from_str(
        r#"
        schema_version = 1
        [tiers.tier1]
        description = "User tier 1"
        models = ["gemini-cli/google/flash/low"]
        [tiers.tier2]
        description = "User tier 2"
        models = ["codex/openai/gpt/medium"]
    "#,
    )
    .unwrap();

    let project_toml: toml::Value = toml::from_str(
        r#"
        schema_version = 1
        [tiers.tier2]
        description = "Project tier 2 override"
        models = ["claude-code/anthropic/opus/high"]
        [tiers.tier3]
        description = "Project tier 3"
        models = ["codex/openai/o3/xhigh"]
    "#,
    )
    .unwrap();

    let merged = merge_toml_values(user_toml, project_toml);
    let config: ProjectConfig = toml::from_str(&toml::to_string(&merged).unwrap()).unwrap();

    // tier1 from user (untouched)
    assert!(config.tiers.contains_key("tier1"));
    assert_eq!(config.tiers["tier1"].description, "User tier 1");

    // tier2 from project (overridden)
    assert!(config.tiers.contains_key("tier2"));
    assert_eq!(config.tiers["tier2"].description, "Project tier 2 override");
    assert_eq!(config.tiers["tier2"].models.len(), 1);
    assert!(config.tiers["tier2"].models[0].contains("claude-code"));

    // tier3 from project (new)
    assert!(config.tiers.contains_key("tier3"));
}

#[test]
fn test_merge_scalar_overlay_wins() {
    let base: toml::Value = toml::from_str(
        r#"
        schema_version = 1
        [project]
        name = "user-default"
        max_recursion_depth = 3
    "#,
    )
    .unwrap();

    let overlay: toml::Value = toml::from_str(
        r#"
        [project]
        name = "my-project"
    "#,
    )
    .unwrap();

    let merged = merge_toml_values(base, overlay);
    let config: ProjectConfig = toml::from_str(&toml::to_string(&merged).unwrap()).unwrap();

    assert_eq!(config.project.name, "my-project");
    // max_recursion_depth should come from user (base) since project didn't set it
    // After merge, the [project] table merges recursively:
    // base has name + max_recursion_depth, overlay has name only
    // So merged [project] has name from overlay + max_recursion_depth from base
    assert_eq!(config.project.max_recursion_depth, 3);
}

#[test]
fn test_merged_schema_version_defaults_when_both_omit() {
    // When neither config specifies schema_version, serde's default should apply
    let dir = tempdir().unwrap();

    let user_dir = dir.path().join("user");
    std::fs::create_dir_all(&user_dir).unwrap();
    let user_path = user_dir.join("config.toml");
    std::fs::write(
        &user_path,
        r#"
        [tiers.tier1]
        description = "User tier"
        models = ["gemini-cli/google/flash/low"]
    "#,
    )
    .unwrap();

    let project_dir = dir.path().join("project").join(".csa");
    std::fs::create_dir_all(&project_dir).unwrap();
    let project_path = project_dir.join("config.toml");
    std::fs::write(
        &project_path,
        r#"
        [project]
        name = "test-project"
    "#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .unwrap();
    // Should get CURRENT_SCHEMA_VERSION from serde default, not 0
    assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
    assert!(config.check_schema_version().is_ok());
}

#[test]
fn test_merged_schema_version_uses_max_when_explicit() {
    // When one config has a higher schema_version, max should be used
    let dir = tempdir().unwrap();

    let user_dir = dir.path().join("user");
    std::fs::create_dir_all(&user_dir).unwrap();
    let user_path = user_dir.join("config.toml");
    std::fs::write(
        &user_path,
        r#"
        schema_version = 1
        [tiers.tier1]
        description = "User tier"
        models = ["gemini-cli/google/flash/low"]
    "#,
    )
    .unwrap();

    let project_dir = dir.path().join("project").join(".csa");
    std::fs::create_dir_all(&project_dir).unwrap();
    let project_path = project_dir.join("config.toml");
    // Project omits schema_version — user's explicit 1 should be preserved
    std::fs::write(
        &project_path,
        r#"
        [project]
        name = "test-project"
    "#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .unwrap();
    assert_eq!(config.schema_version, 1);
}

#[test]
fn test_user_config_path_returns_some() {
    // On a normal system with HOME set, this should return Some
    let path = ProjectConfig::user_config_path();
    if std::env::var("HOME").is_ok() {
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(p.to_string_lossy().contains("cli-sub-agent"));
        assert!(p.to_string_lossy().contains("config.toml"));
    }
    // In containers without HOME, it's OK to return None
}

#[test]
fn test_user_config_template_is_valid() {
    let template = ProjectConfig::user_config_template();
    // Template should contain key sections
    assert!(template.contains("schema_version"));
    assert!(template.contains("[resources]"));
    assert!(template.contains("# [tiers."));
    // Template location comment should point to unified path
    assert!(template.contains("cli-sub-agent/config.toml"));
}

#[test]
fn test_user_config_path_matches_global_config_dir() {
    // After unification, user-level ProjectConfig and GlobalConfig share the same directory.
    if std::env::var("HOME").is_err() {
        return; // Skip in containers
    }
    let user_path = ProjectConfig::user_config_path().unwrap();
    let global_path = crate::GlobalConfig::config_path().unwrap();
    assert_eq!(
        user_path.parent(),
        global_path.parent(),
        "User and global config should share the same directory"
    );
}

#[test]
fn test_load_user_tiers_without_project_config() {
    // When only user-level config exists (no project config),
    // load should return user-level tiers.
    let tmp = tempfile::tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp
        .path()
        .join("nonexistent")
        .join(".csa")
        .join("config.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[tiers.user-tier]
description = "User-level tier"
models = ["codex/openai/o3/medium"]
"#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .expect("Should load from user path");
    assert!(config.tiers.contains_key("user-tier"));
}

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
fn test_global_disable_wins_over_project_enable() {
    // Global config disables gemini-cli; project config enables it.
    // After merge, gemini-cli must remain disabled.
    let tmp = tempfile::tempdir().unwrap();
    let user_path = tmp.path().join("user.toml");
    let project_path = tmp.path().join("project.toml");

    std::fs::write(
        &user_path,
        r#"
schema_version = 1
[tools.gemini-cli]
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
[tools.gemini-cli]
enabled = true
suppress_notify = true
[tools.codex]
enabled = true
suppress_notify = true
[tiers.tier-1-quick]
description = "Quick"
models = ["gemini-cli/google/flash/xhigh"]
"#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .expect("Should load merged config");

    // gemini-cli must be disabled (global wins)
    assert!(
        !config.is_tool_enabled("gemini-cli"),
        "globally-disabled tool must remain disabled after merge"
    );
    // codex stays enabled (both agree)
    assert!(config.is_tool_enabled("codex"));
    // gemini-cli must not be auto-selectable
    assert!(
        !config.is_tool_auto_selectable("gemini-cli"),
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
