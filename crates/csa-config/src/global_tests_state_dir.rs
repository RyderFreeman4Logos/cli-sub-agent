use super::*;

#[test]
fn test_state_dir_defaults_parse_when_section_omitted() {
    let config: GlobalConfig = toml::from_str("").unwrap();
    assert_eq!(config.state_dir.max_size_mb, 0);
    assert_eq!(config.state_dir.scan_interval_seconds, 3600);
    assert_eq!(config.state_dir.on_exceed, StateDirOnExceed::Warn);
    assert!(config.state_dir.is_default());
}

#[test]
fn test_state_dir_parses_all_on_exceed_variants() {
    let config: GlobalConfig = toml::from_str(
        r#"
[state_dir]
max_size_mb = 10240
scan_interval_seconds = 1800
on_exceed = "error"
"#,
    )
    .unwrap();
    assert_eq!(config.state_dir.max_size_mb, 10240);
    assert_eq!(config.state_dir.scan_interval_seconds, 1800);
    assert_eq!(config.state_dir.on_exceed, StateDirOnExceed::Error);
    assert!(!config.state_dir.is_default());
}

#[test]
fn test_state_dir_auto_gc_variant() {
    let config: GlobalConfig = toml::from_str(
        r#"
[state_dir]
max_size_mb = 5120
on_exceed = "auto-gc"
"#,
    )
    .unwrap();
    assert_eq!(config.state_dir.on_exceed, StateDirOnExceed::AutoGc);
}

#[test]
fn test_state_dir_skip_serializing_when_default() {
    let config = GlobalConfig::default();
    let toml_str = toml::to_string(&config).unwrap();
    assert!(
        !toml_str.contains("[state_dir]"),
        "Default state_dir should not appear in TOML: {toml_str}"
    );
}

#[test]
fn test_state_dir_serialized_when_non_default() {
    let mut config = GlobalConfig::default();
    config.state_dir.max_size_mb = 10240;
    let toml_str = toml::to_string(&config).unwrap();
    assert!(
        toml_str.contains("max_size_mb = 10240"),
        "Non-default state_dir should appear in TOML: {toml_str}"
    );
}
