use std::process::Command;

fn csa_cmd(tmp: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    scrub_inherited_csa_env(&mut cmd);
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1");
    cmd
}

fn scrub_inherited_csa_env(cmd: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CSA_") {
            cmd.env_remove(key);
        }
    }
}

#[test]
fn config_show_and_get_redact_project_tool_api_key() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[tools.openai-compat]
api_key = "sk-test-secret-12345"
"#,
    )
    .expect("write project config");

    let show_output = csa_cmd(tmp.path())
        .args(["config", "show"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config show");

    assert!(show_output.status.success(), "config show should exit 0");
    let show_stdout = String::from_utf8_lossy(&show_output.stdout);
    assert!(
        !show_stdout.contains("sk-test-secret-12345"),
        "config show leaked the project tool api key"
    );
    assert!(
        show_stdout.contains("***REDACTED***"),
        "config show should mask the project tool api key"
    );

    let get_output = csa_cmd(tmp.path())
        .args(["config", "get", "tools.openai-compat.api_key"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get tools.openai-compat.api_key");

    assert!(get_output.status.success(), "config get should exit 0");
    let get_stdout = String::from_utf8_lossy(&get_output.stdout);
    assert!(
        !get_stdout.contains("sk-test-secret-12345"),
        "config get leaked the project tool api key"
    );
    assert!(
        get_stdout.contains("***REDACTED***"),
        "config get should mask the project tool api key"
    );
}
