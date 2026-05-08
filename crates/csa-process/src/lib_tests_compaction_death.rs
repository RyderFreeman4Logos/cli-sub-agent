/// Tests for child process death detection while stdout pipe remains open.
///
/// Simulates the claude-code auto-compaction scenario: the tool process exits
/// but a grandchild subprocess inherits stdout, preventing EOF. Without the
/// fix the daemon would block until the idle_timeout fires (minutes/hours).
/// With the fix the watchdog tick detects the zombie PID and breaks immediately.
use super::*;

/// Spawn a script that:
/// 1. Prints a short line to stdout (simulating the compaction message)
/// 2. Forks a grandchild with `sleep N` that inherits stdout (keeping pipe open)
/// 3. The parent exits immediately
///
/// The grandchild is killed by the drop of the returned child; this is fine for
/// the test — we only need the pipe to stay open long enough that the watchdog
/// tick fires (≥200ms) and detects the zombie parent.
fn compaction_death_script(hold_secs: u64) -> String {
    format!(
        r#"printf '{{"type":"system","subtype":"status","status":"compacting"}}\n'
sleep {hold_secs} &
# disown prevents the subshell from printing a "Terminated" message on death
disown
exit 0
"#
    )
}

/// Child exits while grandchild holds stdout open.
/// `wait_and_capture_with_idle_timeout` must detect the exit via try_wait()
/// and return within a few seconds, NOT wait for the idle_timeout.
#[cfg(unix)]
#[tokio::test]
async fn test_compaction_death_detected_before_idle_timeout() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output_path = tmp.path().join("output.log");

    let mut cmd = Command::new("bash");
    cmd.args(["-c", &compaction_death_script(30)]);
    let child = spawn_tool(cmd, None).await.expect("spawn");

    let start = Instant::now();
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        // Very long idle timeout — must NOT be what ends the test.
        Duration::from_secs(7200),
        Duration::from_secs(600),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        Some(&output_path),
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("wait_and_capture");
    let elapsed = start.elapsed();

    // The watchdog tick (200ms) detects the zombie and exits.
    // Allow up to 5s for the kernel to mark the child as zombie + one poll cycle.
    assert!(
        elapsed < Duration::from_secs(5),
        "should detect child death within 5s, elapsed={elapsed:?}"
    );

    // Exit code must be non-zero: CSA treats child death with open pipe as failure.
    assert_ne!(
        result.exit_code, 0,
        "compaction death must yield non-zero exit code, got {}",
        result.exit_code
    );

    // Summary must mention the pipe/compaction scenario.
    assert!(
        result
            .summary
            .contains("exited while stdout pipe still open"),
        "summary should describe compaction death scenario, got: {:?}",
        result.summary
    );

    // Diagnostic must also appear in stderr_output.
    assert!(
        result
            .stderr_output
            .contains("exited while stdout pipe still open"),
        "stderr_output should contain compaction death diagnostic, got: {:?}",
        result.stderr_output
    );
}

/// Verify that Tokio's try_wait() correctly detects process exit.
/// This replaces the former pid_has_exited_or_zombie test, which relied on
/// /proc-based zombie polling and was vulnerable to PID reuse races.
#[tokio::test]
async fn test_try_wait_detects_exit() {
    let mut child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");

    // Process is alive: try_wait should return None.
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "live sleep process should not have exited yet"
    );

    // Kill the process.
    child.start_kill().expect("start_kill");
    // Give the kernel a moment to process the signal.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // After kill, try_wait should return Some (process reaped by Tokio).
    assert!(
        child.try_wait().expect("try_wait").is_some(),
        "killed process should be detected as exited by try_wait()"
    );
}

/// Regression: a child that exits normally (both stdout and stderr EOF) must
/// NOT trigger child_exited_early — the normal exit path handles it.
#[tokio::test]
async fn test_normal_exit_does_not_set_child_exited_early() {
    let mut cmd = Command::new("echo");
    cmd.arg("hello");
    let child = spawn_tool(cmd, None).await.expect("spawn");

    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(30),
        Duration::from_secs(600),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("wait");

    assert_eq!(result.exit_code, 0);
    assert!(result.output.contains("hello"));
    // Must not contain the compaction-death diagnostic note.
    assert!(
        !result
            .stderr_output
            .contains("exited while stdout pipe still open"),
        "normal exit must not set child_exited_early, stderr={:?}",
        result.stderr_output
    );
}
