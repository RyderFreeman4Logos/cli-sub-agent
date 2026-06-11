use super::*;

#[test]
fn test_run_config_large_diff_warning_defaults() {
    let cfg = RunConfig::default();

    assert!(cfg.large_diff_warning.enabled);
    assert_eq!(cfg.large_diff_warning.changed_files, 5);
    assert_eq!(cfg.large_diff_warning.changed_lines, 500);
    assert_eq!(cfg.large_diff_warning.approx_diff_tokens, 8_000);
    assert_eq!(cfg.large_diff_warning.mode, RunLargeDiffWarningMode::Warn);
    assert!(cfg.large_diff_warning.is_default());
}

#[test]
fn test_run_config_deserializes_large_diff_warning() {
    let toml_str = r#"
[large_diff_warning]
enabled = true
changed_files = 3
changed_lines = 200
approx_diff_tokens = 4000
mode = "warn"
"#;
    let cfg: RunConfig = toml::from_str(toml_str).unwrap();

    assert!(cfg.large_diff_warning.enabled);
    assert_eq!(cfg.large_diff_warning.changed_files, 3);
    assert_eq!(cfg.large_diff_warning.changed_lines, 200);
    assert_eq!(cfg.large_diff_warning.approx_diff_tokens, 4_000);
    assert_eq!(cfg.large_diff_warning.mode, RunLargeDiffWarningMode::Warn);
}

#[test]
fn test_run_config_is_default_reflects_large_diff_warning() {
    let cfg = RunConfig {
        large_diff_warning: RunLargeDiffWarningConfig {
            changed_files: 10,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(!cfg.is_default());
}
