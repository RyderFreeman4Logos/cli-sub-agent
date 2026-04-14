use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn write_raw_project_config(dir: &Path, config_toml: &str) -> PathBuf {
    let config_dir = dir.join(".csa");
    fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("config.toml");
    fs::write(&config_path, config_toml).unwrap();
    config_path
}

#[cfg(not(feature = "codex-acp"))]
#[test]
fn test_validate_codex_acp_transport_requires_compiled_feature() {
    let dir = tempdir().unwrap();
    let config_path = write_raw_project_config(
        dir.path(),
        r#"
[tools.codex]
transport = "acp"
"#,
    );

    let err = validate_config_with_paths(None, &config_path).unwrap_err();
    let message = err.to_string();

    assert!(
        message.contains("[tools.codex].transport"),
        "error should point to the exact config key: {message}"
    );
    assert!(
        message.contains("codex-acp"),
        "error should mention the missing cargo feature: {message}"
    );
    assert!(
        message.contains("cargo"),
        "error should include a rebuild command: {message}"
    );
    assert!(
        message.contains("cargo install --path crates/cli-sub-agent --features codex-acp"),
        "error should point to the repo-local install command: {message}"
    );
    assert!(
        message.contains("transport = \"acp\" requires"),
        "error should explain why ACP is rejected: {message}"
    );
}

#[cfg(feature = "codex-acp")]
#[test]
fn test_validate_codex_acp_transport_accepts_feature_build() {
    let dir = tempdir().unwrap();
    let config_path = write_raw_project_config(
        dir.path(),
        r#"
[tools.codex]
transport = "acp"
"#,
    );

    let result = validate_config_with_paths(None, &config_path);
    assert!(
        result.is_ok(),
        "feature build should accept codex ACP transport: {result:?}"
    );
}

#[test]
fn test_validate_codex_cli_transport_override_accepts_all_builds() {
    let dir = tempdir().unwrap();
    let config_path = write_raw_project_config(
        dir.path(),
        r#"
[tools.codex]
transport = "cli"
"#,
    );

    let result = validate_config_with_paths(None, &config_path);
    assert!(
        result.is_ok(),
        "CLI transport should validate in every build: {result:?}"
    );
}

#[test]
fn test_validate_codex_transport_rejects_unknown_value() {
    let dir = tempdir().unwrap();
    let config_path = write_raw_project_config(
        dir.path(),
        r#"
[tools.codex]
transport = "stdio"
"#,
    );

    let err = validate_config_with_paths(None, &config_path).unwrap_err();
    let message = err.to_string();

    assert!(
        message.contains("[tools.codex].transport"),
        "error should point to the exact config key: {message}"
    );
    assert!(
        message.contains("unknown transport \"stdio\" for tool \"codex\""),
        "error should name the bad transport value and tool: {message}"
    );
    assert!(
        message.contains("legal values are: cli, acp"),
        "error should list the accepted transport values: {message}"
    );
}

#[test]
fn test_validate_non_codex_transport_override_rejected() {
    let dir = tempdir().unwrap();
    let config_path = write_raw_project_config(
        dir.path(),
        r#"
[tools.gemini-cli]
transport = "cli"
"#,
    );

    let err = validate_config_with_paths(None, &config_path).unwrap_err();
    let message = err.to_string();

    assert!(
        message.contains("[tools.gemini-cli].transport"),
        "error should point to the exact config key: {message}"
    );
    assert!(
        message.contains("does not support transport override"),
        "error should reject non-codex transport overrides: {message}"
    );
    assert!(
        message.contains("only valid for codex"),
        "error should explain the allowed scope: {message}"
    );
}
