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
fn arch_help_shows_deep_module_options() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["arch", "--help"])
        .output()
        .expect("failed to run csa arch --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Architecture analysis description"));
    assert!(stdout.contains("<DESCRIPTION>"));
    assert!(stdout.contains("--tool"));
    assert!(stdout.contains("--timeout"));
    assert!(stdout.contains("--allow-base-branch-working"));
}
