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
fn merge_help_shows_post_merge_checkout_options() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["merge", "--help"])
        .output()
        .expect("failed to run csa merge --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("PR_NUMBER"));
    assert!(stdout.contains("--rebase"));
    assert!(stdout.contains("--force"));
    assert!(stdout.contains("--skip-pr-bot"));
}
