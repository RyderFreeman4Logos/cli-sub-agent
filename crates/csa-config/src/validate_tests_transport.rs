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

#[test]
fn validate_tool_transport_matrix_matches_phase_3_contract() {
    struct Case {
        tool: &'static str,
        value: &'static str,
        should_pass: bool,
        expected_message: Option<&'static str>,
    }

    let cases = [
        Case {
            tool: "claude-code",
            value: "auto",
            should_pass: true,
            expected_message: None,
        },
        Case {
            tool: "claude-code",
            value: "acp",
            should_pass: true,
            expected_message: None,
        },
        Case {
            tool: "claude-code",
            value: "cli",
            should_pass: true,
            expected_message: None,
        },
        Case {
            tool: "codex",
            value: "auto",
            should_pass: true,
            expected_message: None,
        },
        Case {
            tool: "codex",
            value: "acp",
            should_pass: true,
            expected_message: None,
        },
        Case {
            tool: "codex",
            value: "cli",
            should_pass: false,
            expected_message: Some(
                "codex does not yet support CLI transport — will be added in #643 Phase 4",
            ),
        },
        Case {
            tool: "gemini-cli",
            value: "auto",
            should_pass: true,
            expected_message: None,
        },
        Case {
            tool: "gemini-cli",
            value: "cli",
            should_pass: true,
            expected_message: None,
        },
        Case {
            tool: "gemini-cli",
            value: "acp",
            should_pass: false,
            expected_message: Some("gemini-cli does not support ACP transport"),
        },
        Case {
            tool: "opencode",
            value: "auto",
            should_pass: true,
            expected_message: None,
        },
        Case {
            tool: "opencode",
            value: "cli",
            should_pass: true,
            expected_message: None,
        },
        Case {
            tool: "opencode",
            value: "acp",
            should_pass: false,
            expected_message: Some("opencode does not support ACP transport"),
        },
    ];

    for case in cases {
        let dir = tempdir().unwrap();
        let config_path = write_raw_project_config(
            dir.path(),
            &format!(
                r#"
[tools.{}]
transport = "{}"
"#,
                case.tool, case.value
            ),
        );

        let result = validate_config_with_paths(None, &config_path);
        if case.should_pass {
            assert!(
                result.is_ok(),
                "{}={} should validate, got: {result:?}",
                case.tool,
                case.value
            );
            continue;
        }

        let err = result.expect_err("invalid transport should fail validation");
        let message = format!("{err:#}");
        let key = format!("tools.{}.transport", case.tool);
        let value = format!("\"{}\"", case.value);

        assert!(
            message.contains(&key),
            "error should name the offending key path: {message}"
        );
        assert!(
            message.contains(&value),
            "error should include the offending value: {message}"
        );
        assert!(
            message.contains(
                case.expected_message
                    .expect("expected transport error text")
            ),
            "error should explain the phase-2 transport contract: {message}"
        );
    }
}

#[test]
fn validate_tool_transport_rejects_unknown_value() {
    let dir = tempdir().unwrap();
    let config_path = write_raw_project_config(
        dir.path(),
        r#"
[tools.codex]
transport = "stdio"
"#,
    );

    let err = validate_config_with_paths(None, &config_path).unwrap_err();
    let message = format!("{err:#}");

    assert!(
        message.contains("tools.codex.transport"),
        "error should point to the exact config key: {message}"
    );
    assert!(
        message.contains("\"stdio\""),
        "error should name the bad transport value: {message}"
    );
    assert!(
        message.contains("legal values are: auto, acp, cli"),
        "error should list the accepted transport values: {message}"
    );
}
