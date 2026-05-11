use super::*;

#[test]
fn test_github_defaults_parse_when_section_omitted() {
    let config: GlobalConfig = toml::from_str("").unwrap();
    assert_eq!(config.github.config_dir, None);
}

#[test]
fn test_github_config_dir_parses_from_global_config() {
    let config: GlobalConfig = toml::from_str(
        r#"
[github]
config_dir = "/tmp/gh-aider"
"#,
    )
    .unwrap();
    assert_eq!(config.github.config_dir.as_deref(), Some("/tmp/gh-aider"));
}

#[test]
fn test_default_template_contains_github_section() {
    let template = GlobalConfig::default_template();
    assert!(template.contains("[github]"));
    assert!(template.contains("config_dir"));
}
