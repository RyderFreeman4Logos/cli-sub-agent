use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn csa() -> &'static str {
    env!("CARGO_BIN_EXE_csa")
}

fn write_project(root: &std::path::Path, declare: bool) {
    let config_dir = root.join(".csa");
    fs::create_dir_all(&config_dir).unwrap();
    let entry = if declare {
        r#"
[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "config-only-fake"
reasoning_efforts = ["high"]
allow_custom_reasoning = false
"#
    } else {
        ""
    };
    fs::write(
        config_dir.join("config.toml"),
        format!(
            r#"
[project]
name = "model-catalog-e2e"

[model_catalog]
mode = "replace"
closed = true
{entry}
[tiers.test]
description = "catalog test"
models = ["codex/openai/config-only-fake/high"]

[tier_mapping]
default = "test"

[review]
tier = "test"

[debate]
tier = "test"
"#
        ),
    )
    .unwrap();
}

fn isolated_command(root: &std::path::Path) -> Command {
    let mut command = Command::new(csa());
    command
        .current_dir(root)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("xdg"));
    command
}

#[test]
fn validate_and_doctor_agree_for_config_only_model() {
    let temp = tempdir().unwrap();
    write_project(temp.path(), true);

    let validate = isolated_command(temp.path())
        .args(["config", "validate", "--cd", temp.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );

    let doctor = isolated_command(temp.path())
        .args(["--format", "json", "doctor", "routing"])
        .output()
        .unwrap();
    assert!(
        doctor.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let model = &json["routing"][0]["models"][0];
    assert_eq!(model["model"], "config-only-fake");
    assert_eq!(model["catalog_valid"], true);
    assert!(
        model["catalog_source"]
            .as_str()
            .unwrap()
            .contains(".csa/config.toml")
    );
    assert!(
        model["admission_status"]
            .as_str()
            .unwrap()
            .starts_with("admitted")
    );
}

#[test]
fn validate_defers_configured_unknown_model_warning_while_doctor_reports_it() {
    let temp = tempdir().unwrap();
    write_project(temp.path(), false);

    let validate = isolated_command(temp.path())
        .args(["config", "validate", "--cd", temp.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(validate.status.success());
    let validation_stderr = String::from_utf8_lossy(&validate.stderr);
    assert!(
        !validation_stderr.contains("warning"),
        "validation must defer unverified-model diagnostics until dispatch: {validation_stderr}"
    );

    let doctor = isolated_command(temp.path())
        .args(["--format", "json", "doctor", "routing"])
        .output()
        .unwrap();
    assert!(doctor.status.success());
    let json: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(json["routing"][0]["models"][0]["catalog_valid"], true);
    assert!(
        json["routing"][0]["models"][0]["admission_status"]
            .as_str()
            .unwrap()
            .contains("warning")
    );
}
