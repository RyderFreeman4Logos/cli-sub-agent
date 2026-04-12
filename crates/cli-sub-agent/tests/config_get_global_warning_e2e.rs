use std::path::Path;
use std::process::Command;

fn csa_cmd(tmp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"));
    cmd
}

#[test]
fn config_get_global_warns_when_falling_back_to_raw_invalid_global_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let global_dir = tmp.path().join(".config/cli-sub-agent");
    std::fs::create_dir_all(&global_dir).expect("create global config dir");
    std::fs::write(
        global_dir.join("config.toml"),
        r#"
[review]
tool = "auto"

[defaults]
max_concurrent = "bad"
"#,
    )
    .expect("write global config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "review.tool", "--global"])
        .current_dir(tmp.path())
        .output()
        .expect("run csa config get");

    assert!(
        output.status.success(),
        "config get should still succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "auto");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("warning: global config has parse errors; showing raw value"),
        "stderr should surface the raw-value fallback warning, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
