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
fn test_suppress_notify_in_tool_config() {
    let toml_str = r#"
        schema_version = 1
        [tools.codex]
        enabled = true
        suppress_notify = true
        [tools.gemini-cli]
        enabled = true
    "#;
    let config: ProjectConfig = toml::from_str(toml_str).unwrap();

    assert!(config.should_suppress_codex_notify());
    // gemini-cli doesn't have suppress_notify set, should default to false
    assert!(!config.tools["gemini-cli"].suppress_notify);
}

#[test]
fn test_suppress_notify_default_false() {
    let toml_str = r#"
        schema_version = 1
        [tools.codex]
        enabled = true
    "#;
    let config: ProjectConfig = toml::from_str(toml_str).unwrap();
    assert!(!config.should_suppress_codex_notify());
}

#[test]
fn test_user_config_path_returns_some() {
    // On a normal system with HOME set, this should return Some
    let path = ProjectConfig::user_config_path();
    if std::env::var("HOME").is_ok() {
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(p.to_string_lossy().contains("csa"));
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
    assert!(template.contains("suppress_notify"));
    assert!(template.contains("# [tiers."));
}
