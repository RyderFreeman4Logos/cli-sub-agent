use std::process::Command;

fn csa_cmd(tmp: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1");
    cmd
}

#[test]
fn config_show_renders_resolved_tool_transport() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
[project]
name = "test-project"

[tools.codex]
transport = "acp"
"#,
    )
    .expect("write config");

    let project_root = tmp.path().display().to_string();
    let output = csa_cmd(tmp.path())
        .args(["config", "show", "--cd", &project_root])
        .output()
        .expect("failed to run csa config show --cd");

    assert!(output.status.success(), "csa config show should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[tools.codex]"));
    assert!(stdout.contains("transport = \"acp\""));
}

#[test]
fn config_show_renders_valid_claude_code_cli_transport() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
[project]
name = "test-project"

[tools.claude-code]
transport = "cli"
"#,
    )
    .expect("write config");

    let project_root = tmp.path().display().to_string();
    let output = csa_cmd(tmp.path())
        .args(["config", "show", "--cd", &project_root])
        .output()
        .expect("failed to run csa config show --cd");

    assert!(output.status.success(), "csa config show should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[tools.claude-code]"));
    assert!(stdout.contains("transport = \"cli\""));
}
