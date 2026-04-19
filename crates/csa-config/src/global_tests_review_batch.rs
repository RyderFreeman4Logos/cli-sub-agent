use super::*;

#[test]
fn test_review_config_gate_fields_default() {
    let config = ReviewConfig::default();
    assert!(config.gate_command.is_none());
    assert_eq!(config.gate_timeout_secs, 250);
    assert_eq!(config.batch_commits, 1);
}

#[test]
fn test_review_config_gate_timeout_default_skipped_in_serialization() {
    let config = ReviewConfig::default();
    let toml_str = toml::to_string(&config).unwrap();
    assert!(
        !toml_str.contains("batch_commits"),
        "Default batch_commits should be omitted from TOML output"
    );
    assert!(
        !toml_str.contains("gate_timeout_secs"),
        "Default gate_timeout_secs should be omitted from TOML output"
    );
    assert!(
        !toml_str.contains("gate_command"),
        "None gate_command should be omitted from TOML output"
    );
}

#[test]
fn test_review_config_batch_commits_default_when_absent() {
    let toml_str = r#"
[review]
tool = "auto"
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.review.batch_commits, 1);
}

#[test]
fn test_review_config_batch_commits_parses() {
    let toml_str = r#"
[review]
tool = "auto"
batch_commits = 5
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.review.batch_commits, 5);
}

#[test]
fn test_review_config_batch_commits_non_default_serialized() {
    let config = ReviewConfig {
        batch_commits: 5,
        ..Default::default()
    };
    let toml_str = toml::to_string(&config).unwrap();
    assert!(
        toml_str.contains("batch_commits = 5"),
        "Non-default batch_commits should appear in TOML output"
    );
}
