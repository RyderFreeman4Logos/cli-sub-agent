use std::path::Path;
use std::process::Command;

fn csa_cmd(tmp: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1");
    cmd
}

fn global_config_path(tmp: &Path) -> std::path::PathBuf {
    if cfg!(target_os = "macos") {
        tmp.join("Library/Application Support/cli-sub-agent/config.toml")
    } else {
        tmp.join(".config/cli-sub-agent/config.toml")
    }
}

#[test]
fn config_get_state_dir_keys_require_explicit_global_values() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let global_config_path = global_config_path(tmp.path());
    let global_dir = global_config_path.parent().expect("global config dir");
    std::fs::create_dir_all(global_dir).expect("create global config dir");
    std::fs::write(
        &global_config_path,
        r#"
[state_dir]
max_size_mb = 1024
"#,
    )
    .expect("write global config");

    let max_size_output = csa_cmd(tmp.path())
        .args(["config", "get", "state_dir.max_size_mb"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get state_dir.max_size_mb");
    assert!(max_size_output.status.success(), "config get should exit 0");
    assert_eq!(
        String::from_utf8_lossy(&max_size_output.stdout).trim(),
        "1024"
    );

    for key in ["state_dir.on_exceed", "state_dir.scan_interval_seconds"] {
        let output = csa_cmd(tmp.path())
            .args(["config", "get", key])
            .current_dir(tmp.path())
            .output()
            .unwrap_or_else(|_| panic!("failed to run csa config get {key}"));

        assert!(
            !output.status.success(),
            "config get for {key} should fail when the key is not explicitly set"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("Key not found: {key}")),
            "stderr should report missing explicit key for {key}, got: {stderr}"
        );
    }
}
