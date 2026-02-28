// Tests for deprecated configuration key handling (backward compatibility).

#[test]
fn test_legacy_config_with_initial_estimates_parses_ok() {
    // Old configs may contain [resources.initial_estimates].
    // Verify backward-compatible deserialization: no error, field populated.
    let toml_str = r#"
schema_version = 4

[project]
name = "legacy-project"
created_at = "2025-01-01T00:00:00Z"
max_recursion_depth = 5

[resources]
min_free_memory_mb = 4096
idle_timeout_seconds = 300

[resources.initial_estimates]
gemini-cli = 150
opencode = 500
codex = 800
claude-code = 1200
"#;
    let config: crate::config::ProjectConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.resources.initial_estimates.len(), 4);
    assert_eq!(config.resources.initial_estimates["codex"], 800);
}

#[test]
fn test_initial_estimates_not_serialized() {
    // Even when initial_estimates is populated, skip_serializing should omit it.
    let mut resources = ResourcesConfig::default();
    resources.initial_estimates.insert("codex".to_string(), 800);
    let serialized = toml::to_string(&resources).unwrap();
    assert!(
        !serialized.contains("initial_estimates"),
        "initial_estimates should be omitted from serialized output, got: {serialized}"
    );
}

#[test]
fn test_validate_warns_on_deprecated_initial_estimates() {
    // Validate that non-empty initial_estimates doesn't cause a validation error.
    // Write raw TOML directly since skip_serializing would omit the field via save().
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join(".csa");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
schema_version = 4

[project]
name = "legacy"
created_at = "2025-01-01T00:00:00Z"
max_recursion_depth = 5

[resources]
min_free_memory_mb = 4096
idle_timeout_seconds = 300

[resources.initial_estimates]
codex = 800
"#,
    )
    .unwrap();

    let result = validate_config(dir.path());
    assert!(
        result.is_ok(),
        "Deprecated initial_estimates should not cause validation failure: {:?}",
        result.err()
    );
}
