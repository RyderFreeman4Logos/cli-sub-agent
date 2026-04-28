use std::path::Path;
use std::process::Command;

fn csa_cmd(tmp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1");
    cmd
}

#[test]
fn config_get_resolves_vcs_snapshot_defaults_and_explicit_values() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(&config_path, "schema_version = 1\n").expect("write default config");

    let default_auto_snapshot = csa_cmd(tmp.path())
        .args(["config", "get", "vcs.auto_snapshot"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get vcs.auto_snapshot");
    assert!(
        default_auto_snapshot.status.success(),
        "config get vcs.auto_snapshot should exit 0"
    );
    assert_eq!(
        String::from_utf8_lossy(&default_auto_snapshot.stdout).trim(),
        "false"
    );

    let default_trigger = csa_cmd(tmp.path())
        .args(["config", "get", "vcs.snapshot_trigger"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get vcs.snapshot_trigger");
    assert!(
        default_trigger.status.success(),
        "config get vcs.snapshot_trigger should exit 0"
    );
    assert_eq!(
        String::from_utf8_lossy(&default_trigger.stdout).trim(),
        "post-run"
    );

    std::fs::write(
        &config_path,
        r#"
schema_version = 1

[vcs]
auto_snapshot = true
snapshot_trigger = "tool-completed"
"#,
    )
    .expect("write explicit vcs config");

    let explicit_auto_snapshot = csa_cmd(tmp.path())
        .args(["config", "get", "vcs.auto_snapshot"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get explicit vcs.auto_snapshot");
    assert!(
        explicit_auto_snapshot.status.success(),
        "config get explicit vcs.auto_snapshot should exit 0"
    );
    assert_eq!(
        String::from_utf8_lossy(&explicit_auto_snapshot.stdout).trim(),
        "true"
    );

    let explicit_trigger = csa_cmd(tmp.path())
        .args(["config", "get", "vcs.snapshot_trigger"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get explicit vcs.snapshot_trigger");
    assert!(
        explicit_trigger.status.success(),
        "config get explicit vcs.snapshot_trigger should exit 0"
    );
    assert_eq!(
        String::from_utf8_lossy(&explicit_trigger.stdout).trim(),
        "tool-completed"
    );
}
