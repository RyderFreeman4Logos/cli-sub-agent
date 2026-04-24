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
fn config_set_get_show_round_trips_primary_writer_spec() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let set_output = csa_cmd(tmp.path())
        .args([
            "config",
            "set",
            "preferences.primary_writer_spec",
            "codex/openai/gpt-5.4/high",
        ])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config set");
    assert!(
        set_output.status.success(),
        "config set should exit 0, stderr: {}",
        String::from_utf8_lossy(&set_output.stderr)
    );

    let get_output = csa_cmd(tmp.path())
        .args(["config", "get", "preferences.primary_writer_spec"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get");
    assert!(get_output.status.success(), "config get should exit 0");
    assert_eq!(
        String::from_utf8_lossy(&get_output.stdout).trim(),
        "codex/openai/gpt-5.4/high"
    );

    let show_output = csa_cmd(tmp.path())
        .args(["config", "show"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config show");
    assert!(show_output.status.success(), "config show should exit 0");
    let stdout = String::from_utf8_lossy(&show_output.stdout);
    assert!(stdout.contains("primary_writer_spec = \"codex/openai/gpt-5.4/high\""));
}
