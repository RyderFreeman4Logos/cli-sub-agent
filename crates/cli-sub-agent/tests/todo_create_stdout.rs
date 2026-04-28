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
fn text_stdout_is_timestamp_only() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let output = csa_cmd(tmp.path())
        .args(["todo", "create", "--no-branch", "stdout contract"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa todo create");

    assert!(
        output.status.success(),
        "csa todo create should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    let stdout_lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        stdout_lines.len(),
        1,
        "stdout should contain only the todo timestamp, got: {stdout:?}"
    );
    assert!(
        stdout_lines[0]
            .chars()
            .all(|ch| ch.is_ascii_digit() || ch == 'T'),
        "stdout should be the todo timestamp, got: {stdout:?}"
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains("Created TODO plan: stdout contract"),
        "stderr should contain the human-readable creation message, got: {stderr:?}"
    );
    assert!(
        stderr.contains("Path:"),
        "stderr should contain the human-readable plan path, got: {stderr:?}"
    );
}
