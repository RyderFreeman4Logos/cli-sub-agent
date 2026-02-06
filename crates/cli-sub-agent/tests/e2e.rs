// End-to-end tests for the csa binary.
// Requires actual tool installations for full testing.

#[test]
fn cli_help_displays_correctly() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_csa"))
        .arg("--help")
        .output()
        .expect("failed to run csa --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("CLI Sub-Agent"));
    assert!(stdout.contains("run"));
    assert!(stdout.contains("session"));
    assert!(stdout.contains("init"));
    assert!(stdout.contains("gc"));
    assert!(stdout.contains("config"));
}

#[test]
fn run_help_shows_tool_options() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_csa"))
        .args(["run", "--help"])
        .output()
        .expect("failed to run csa run --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--tool"));
    assert!(stdout.contains("--session"));
    assert!(stdout.contains("--ephemeral"));
    assert!(stdout.contains("--model"));
}

#[test]
fn review_help_shows_options() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_csa"))
        .args(["review", "--help"])
        .output()
        .expect("failed to run csa review --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Review code changes using an AI tool"));
    assert!(stdout.contains("--tool"));
    assert!(stdout.contains("--session"));
    assert!(stdout.contains("--diff"));
    assert!(stdout.contains("--branch"));
    assert!(stdout.contains("--commit"));
    assert!(stdout.contains("--model"));
}
