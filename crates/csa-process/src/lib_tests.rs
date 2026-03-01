use super::*;

#[test]
fn test_extract_summary_empty() {
    assert_eq!(extract_summary(""), "");
}

#[test]
fn test_extract_summary_single_line() {
    assert_eq!(extract_summary("Hello, world!"), "Hello, world!");
}

#[test]
fn test_extract_summary_multi_line() {
    let input = "First line\nSecond line\nThird line";
    assert_eq!(extract_summary(input), "Third line");
}

#[test]
fn test_extract_summary_with_empty_lines() {
    let input = "First line\n\nThird line\n\n";
    assert_eq!(extract_summary(input), "Third line");
}

#[test]
fn test_extract_summary_long_line() {
    let long = "a".repeat(250);
    let summary = extract_summary(&long);
    assert_eq!(summary.chars().count(), 200);
    assert!(summary.ends_with("..."));
    assert_eq!(summary.strip_suffix("...").unwrap(), &long[..197]);
}

#[test]
fn test_extract_summary_exactly_200_chars() {
    let exact = "a".repeat(200);
    let summary = extract_summary(&exact);
    assert_eq!(summary.chars().count(), 200);
    assert!(!summary.ends_with("..."));
}

#[test]
fn test_extract_summary_multibyte_truncation() {
    // Create a string where truncation would fall in the middle of multi-byte UTF-8 characters.
    // Emoji character '🔥' is 4 bytes (F0 9F 94 A5 in UTF-8).
    // We need more than 200 characters total to trigger truncation.

    // Use 196 ASCII chars + 10 emoji chars = 206 chars total
    let mut long_line = "a".repeat(196);
    for _ in 0..10 {
        long_line.push('🔥');
    }

    // Total: 196 + 10 = 206 chars (but many more bytes due to emoji)
    assert_eq!(long_line.chars().count(), 206);

    // This should NOT panic, even with multi-byte characters
    let summary = extract_summary(&long_line);

    // Summary should be 197 chars + "..." = 200 chars total
    assert_eq!(summary.chars().count(), 200);
    assert!(summary.ends_with("..."));

    // The truncated part should be exactly 197 characters
    let content_without_ellipsis = summary.strip_suffix("...").unwrap();
    assert_eq!(content_without_ellipsis.chars().count(), 197);

    // Should have the first 196 'a' chars and the first emoji character
    assert!(content_without_ellipsis.starts_with(&"a".repeat(196)));
    assert!(content_without_ellipsis.ends_with('🔥'));
}

#[test]
fn test_execution_result_construction() {
    let result = ExecutionResult {
        output: "test output".to_string(),
        stderr_output: String::new(),
        summary: "test summary".to_string(),
        exit_code: 0,
    };
    assert_eq!(result.output, "test output");
    assert_eq!(result.summary, "test summary");
    assert_eq!(result.exit_code, 0);
    assert!(result.stderr_output.is_empty());
}

#[tokio::test]
async fn test_spawn_tool_returns_valid_child() {
    let mut cmd = Command::new("echo");
    cmd.arg("test");

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn tool");
    let pid = child.id().expect("Child process has no PID");

    // PID should be a positive number
    assert!(pid > 0);

    // Clean up by waiting for the child
    let result = wait_and_capture(child, StreamMode::BufferOnly)
        .await
        .expect("Failed to wait for child");
    assert_eq!(result.exit_code, 0);
    assert!(result.output.contains("test"));
}

#[tokio::test]
async fn test_spawn_tool_with_none_stdin_uses_null_stdin() {
    let cmd = Command::new("cat");
    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture(child, StreamMode::BufferOnly)
        .await
        .expect("Failed to wait");

    assert_eq!(result.exit_code, 0);
    assert!(
        result.output.is_empty(),
        "cat should receive EOF immediately with null stdin"
    );
}

#[tokio::test]
async fn test_spawn_tool_with_some_stdin_writes_input() {
    let cmd = Command::new("cat");
    let payload = b"stdin-payload\n".to_vec();

    let child = spawn_tool(cmd, Some(payload.clone()))
        .await
        .expect("Failed to spawn");
    let result = wait_and_capture(child, StreamMode::BufferOnly)
        .await
        .expect("Failed to wait");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.output, String::from_utf8(payload).unwrap());
}

#[tokio::test]
async fn test_run_and_capture_still_works() {
    let mut cmd = Command::new("echo");
    cmd.arg("backward_compatible");

    let result = run_and_capture(cmd)
        .await
        .expect("run_and_capture should work");

    assert_eq!(result.exit_code, 0);
    assert!(result.output.contains("backward_compatible"));
}

#[tokio::test]
async fn test_wait_and_capture_with_idle_timeout_kills_silent_process() {
    let mut cmd = Command::new("bash");
    cmd.args(["-c", "sleep 5"]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(1),
        Duration::from_secs(1),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
    )
    .await
    .expect("Failed to wait");

    assert_eq!(result.exit_code, 137);
    assert!(result.summary.contains("idle timeout"));
}

#[tokio::test]
async fn test_wait_and_capture_with_idle_timeout_without_session_dir_does_not_wait_liveness_dead_timeout()
 {
    let mut cmd = Command::new("bash");
    cmd.args(["-c", "sleep 30"]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let start = Instant::now();
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(1),
        Duration::from_secs(DEFAULT_LIVENESS_DEAD_SECS),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
    )
    .await
    .expect("Failed to wait");
    let elapsed = start.elapsed();

    assert_eq!(result.exit_code, 137);
    assert!(
        elapsed < Duration::from_secs(8),
        "session_dir=None should terminate near idle-timeout, elapsed={elapsed:?}"
    );
}

#[tokio::test]
async fn test_wait_and_capture_with_idle_timeout_allows_periodic_output() {
    let mut cmd = Command::new("bash");
    cmd.args(["-c", "for _ in 1 2 3; do echo tick; sleep 0.4; done"]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(1),
        Duration::from_secs(1),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
    )
    .await
    .expect("Failed to wait");

    assert_eq!(result.exit_code, 0);
    assert!(result.output.contains("tick"));
}

#[tokio::test]
async fn test_idle_timeout_detects_partial_output_without_newlines() {
    // Subprocess outputs dots without newlines (like a progress bar).
    // This must NOT trigger idle timeout — any bytes should reset the timer.
    let mut cmd = Command::new("bash");
    cmd.args([
        "-c",
        r#"for _ in 1 2 3 4; do printf "."; sleep 0.3; done; echo done"#,
    ]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(1),
        Duration::from_secs(1),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
    )
    .await
    .expect("Failed to wait");

    assert_eq!(
        result.exit_code, 0,
        "Process should NOT be killed by idle timeout when producing partial output"
    );
    assert!(
        result.output.contains("....done"),
        "Output should contain dots followed by 'done', got: {:?}",
        result.output
    );
}

#[test]
fn test_tool_liveness_true_for_live_pid_and_output_growth() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    std::fs::create_dir_all(&locks_dir).expect("create locks dir");
    std::fs::write(
        locks_dir.join("codex.lock"),
        format!("{{\"pid\": {}}}", std::process::id()),
    )
    .expect("write lock");

    let output_path = tmp.path().join("output.log");
    std::fs::write(&output_path, "a").expect("seed output");
    assert!(ToolLiveness::is_alive(tmp.path()));
    std::fs::write(&output_path, "ab").expect("grow output");
    assert!(ToolLiveness::is_alive(tmp.path()));
}

#[test]
fn test_tool_liveness_false_when_no_signals() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(!ToolLiveness::is_alive(tmp.path()));
}

#[tokio::test]
async fn test_idle_timeout_enters_liveness_mode_before_kill() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    std::fs::create_dir_all(&locks_dir).expect("create locks dir");
    std::fs::write(
        locks_dir.join("codex.lock"),
        format!("{{\"pid\": {}}}", std::process::id()),
    )
    .expect("write lock");

    let mut cmd = Command::new("bash");
    cmd.args(["-c", "sleep 2"]);
    let child = spawn_tool(cmd, None).await.expect("spawn");
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(1),
        Duration::from_secs(1),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        Some(&tmp.path().join("output.log")),
    )
    .await
    .expect("wait");

    assert_eq!(result.exit_code, 0);
}

#[test]
fn test_liveness_true_resets_death_timer() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    std::fs::create_dir_all(&locks_dir).expect("create locks dir");
    std::fs::write(
        locks_dir.join("codex.lock"),
        format!("{{\"pid\": {}}}", std::process::id()),
    )
    .expect("write lock");

    let mut dead_since = Some(Instant::now() - Duration::from_secs(5));
    let mut next_poll = Some(Instant::now() - Duration::from_secs(1));
    let mut last_activity = Instant::now() - Duration::from_secs(2);
    let before = last_activity;
    let terminate = should_terminate_for_idle(
        &mut last_activity,
        Duration::from_secs(1),
        Duration::from_secs(1),
        Some(tmp.path()),
        &mut dead_since,
        &mut next_poll,
    );

    assert!(!terminate);
    assert!(
        dead_since.is_none(),
        "liveness=true should reset death timer"
    );
    assert!(
        last_activity > before,
        "liveness=true should reset idle timer"
    );
}

#[test]
fn test_default_liveness_dead_timeout_constant() {
    assert_eq!(DEFAULT_LIVENESS_DEAD_SECS, 600);
}

#[tokio::test]
async fn test_stderr_capture() {
    // Use bash -c to write to both stdout and stderr
    let mut cmd = Command::new("bash");
    cmd.args(["-c", "echo stdout_line && echo stderr_line >&2"]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture(child, StreamMode::BufferOnly)
        .await
        .expect("Failed to wait");

    assert_eq!(result.exit_code, 0);
    assert!(result.output.contains("stdout_line"));
    assert!(result.stderr_output.contains("stderr_line"));
}

// --- failure_summary tests ---

#[test]
fn test_failure_summary_prefers_stdout() {
    let summary = failure_summary("stdout error\n", "stderr error\n", 1);
    assert_eq!(summary, "stdout error");
}

#[test]
fn test_failure_summary_falls_back_to_stderr() {
    let summary = failure_summary("", "stderr error message\n", 1);
    assert_eq!(summary, "stderr error message");
}

#[test]
fn test_failure_summary_falls_back_to_stderr_when_stdout_whitespace_only() {
    let summary = failure_summary("  \n\n", "stderr msg\n", 42);
    assert_eq!(summary, "stderr msg");
}

#[test]
fn test_failure_summary_exit_code_fallback() {
    let summary = failure_summary("", "", 137);
    assert_eq!(summary, "exit code 137");
}

#[test]
fn test_failure_summary_exit_code_when_both_whitespace() {
    let summary = failure_summary("  \n", "  \n", 2);
    assert_eq!(summary, "exit code 2");
}

#[test]
fn test_failure_summary_truncates_long_stderr() {
    let long_err = "e".repeat(250);
    let summary = failure_summary("", &long_err, 1);
    assert_eq!(summary.chars().count(), 200);
    assert!(summary.ends_with("..."));
}

#[tokio::test]
async fn test_failed_command_uses_stderr_summary() {
    // Command that writes to stderr and exits non-zero
    let mut cmd = Command::new("bash");
    cmd.args(["-c", "echo 'fatal: something went wrong' >&2; exit 1"]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture(child, StreamMode::BufferOnly)
        .await
        .expect("Failed to wait");

    assert_eq!(result.exit_code, 1);
    assert_eq!(result.summary, "fatal: something went wrong");
}

#[tokio::test]
async fn test_failed_command_exit_code_only() {
    // Command that produces no output and exits non-zero
    let mut cmd = Command::new("bash");
    cmd.args(["-c", "exit 42"]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture(child, StreamMode::BufferOnly)
        .await
        .expect("Failed to wait");

    assert_eq!(result.exit_code, 42);
    assert_eq!(result.summary, "exit code 42");
}

// --- helper function tests ---

#[test]
fn test_last_non_empty_line() {
    assert_eq!(last_non_empty_line(""), "");
    assert_eq!(last_non_empty_line("hello"), "hello");
    assert_eq!(last_non_empty_line("a\nb\nc\n"), "c");
    assert_eq!(last_non_empty_line("  \n  \n"), "");
    assert_eq!(last_non_empty_line("first\n\nlast\n\n"), "last");
}

#[test]
fn test_truncate_line() {
    assert_eq!(truncate_line("short", 200), "short");
    assert_eq!(truncate_line("", 200), "");
    let long = "x".repeat(250);
    let result = truncate_line(&long, 200);
    assert_eq!(result.chars().count(), 200);
    assert!(result.ends_with("..."));
}

// --- check_tool_installed tests ---

#[tokio::test]
async fn test_check_tool_installed_with_echo() {
    // `echo` is always available on Linux
    let result = check_tool_installed("echo").await;
    assert!(result.is_ok(), "echo should be found in PATH");
}

#[tokio::test]
async fn test_check_tool_installed_with_nonexistent_tool() {
    let result = check_tool_installed("nonexistent_tool_xyz_12345").await;
    assert!(result.is_err(), "non-existent tool should return error");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not installed"),
        "error should mention 'not installed', got: {err_msg}"
    );
}

#[tokio::test]
async fn test_check_tool_installed_with_true_command() {
    // `true` is a standard POSIX command, always available
    let result = check_tool_installed("true").await;
    assert!(result.is_ok(), "true should be found in PATH");
}

// --- failure_summary priority chain: exhaustive combinations ---

#[test]
fn test_failure_summary_priority_stdout_over_stderr_over_exit_code() {
    // All three sources present: stdout wins
    assert_eq!(
        failure_summary("stdout msg\n", "stderr msg\n", 1),
        "stdout msg"
    );

    // stdout empty, stderr present: stderr wins
    assert_eq!(failure_summary("", "stderr msg\n", 1), "stderr msg");

    // Both empty: exit code fallback
    assert_eq!(failure_summary("", "", 1), "exit code 1");
}

#[test]
fn test_failure_summary_multiline_stdout_uses_last_line() {
    let summary = failure_summary("first\nsecond\nthird\n", "err\n", 1);
    assert_eq!(summary, "third");
}

#[test]
fn test_failure_summary_multiline_stderr_uses_last_line() {
    let summary = failure_summary("", "err1\nerr2\nerr3\n", 1);
    assert_eq!(summary, "err3");
}

#[test]
fn test_failure_summary_various_exit_codes() {
    assert_eq!(failure_summary("", "", 0), "exit code 0");
    assert_eq!(failure_summary("", "", 1), "exit code 1");
    assert_eq!(failure_summary("", "", 127), "exit code 127");
    assert_eq!(failure_summary("", "", 255), "exit code 255");
}

// --- error / boundary path tests ---

#[tokio::test]
async fn test_run_and_capture_nonexistent_command() {
    let cmd = Command::new("nonexistent_binary_xyz_99999");
    let result = run_and_capture(cmd).await;
    assert!(result.is_err(), "spawning a nonexistent binary should fail");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Failed to spawn"),
        "error should mention spawn failure, got: {err_msg}"
    );
}

#[tokio::test]
async fn test_run_and_capture_empty_output() {
    // `true` produces no output and exits 0
    let mut cmd = Command::new("true");
    cmd.args::<[&str; 0], &str>([]);

    let result = run_and_capture(cmd).await.expect("true should succeed");
    assert_eq!(result.exit_code, 0);
    assert!(result.output.is_empty());
    assert_eq!(result.summary, "");
}

#[tokio::test]
async fn test_run_and_capture_false_command() {
    // `false` exits with code 1, no output
    let cmd = Command::new("false");

    let result = run_and_capture(cmd)
        .await
        .expect("false should not error on spawn");
    assert_eq!(result.exit_code, 1);
    assert!(result.output.is_empty());
    assert_eq!(result.summary, "exit code 1");
}

#[test]
fn test_truncate_line_boundary_at_max_plus_one() {
    // Exactly max_chars + 1: should trigger truncation
    let s = "b".repeat(201);
    let result = truncate_line(&s, 200);
    assert_eq!(result.chars().count(), 200);
    assert!(result.ends_with("..."));
}

#[test]
fn test_last_non_empty_line_only_whitespace_lines() {
    assert_eq!(last_non_empty_line("\n\n\n"), "");
    assert_eq!(last_non_empty_line("   \n\t\n  \n"), "");
}

// --- StreamMode tests ---

#[test]
fn test_stream_mode_default_is_tee_to_stderr() {
    let mode: StreamMode = Default::default();
    assert_eq!(mode, StreamMode::TeeToStderr);
}

#[test]
fn test_stream_mode_clone_copy_eq() {
    let a = StreamMode::TeeToStderr;
    let b = a; // Copy
    let c = a.clone(); // Clone
    assert_eq!(a, b);
    assert_eq!(a, c);
    assert_ne!(StreamMode::BufferOnly, StreamMode::TeeToStderr);
}

#[test]
fn test_stream_mode_debug_format() {
    assert_eq!(format!("{:?}", StreamMode::BufferOnly), "BufferOnly");
    assert_eq!(format!("{:?}", StreamMode::TeeToStderr), "TeeToStderr");
}

#[tokio::test]
async fn test_buffer_only_captures_stdout_without_tee() {
    let mut cmd = Command::new("echo");
    cmd.arg("captured-only");

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture(child, StreamMode::BufferOnly)
        .await
        .expect("Failed to wait");

    assert_eq!(result.exit_code, 0);
    assert!(result.output.contains("captured-only"));
}

#[tokio::test]
async fn test_tee_to_stderr_still_captures_stdout() {
    // TeeToStderr should tee to stderr AND capture stdout in result.output
    let mut cmd = Command::new("echo");
    cmd.arg("tee-test");

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture(child, StreamMode::TeeToStderr)
        .await
        .expect("Failed to wait");

    assert_eq!(result.exit_code, 0);
    assert!(
        result.output.contains("tee-test"),
        "TeeToStderr must still capture stdout in result.output"
    );
}

#[tokio::test]
async fn test_run_and_capture_with_stdin_passes_stream_mode() {
    let cmd = Command::new("cat");
    let payload = b"stream-mode-test\n".to_vec();

    let result = run_and_capture_with_stdin(cmd, Some(payload), StreamMode::BufferOnly)
        .await
        .expect("Failed");

    assert_eq!(result.exit_code, 0);
    assert!(result.output.contains("stream-mode-test"));
}

// --- idle timeout + liveness integration tests ---

#[test]
fn test_should_terminate_resets_last_activity_on_liveness_true() {
    // When liveness returns true, both the death timer AND the idle timer
    // (last_activity) should be reset, giving the tool another full window.
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    std::fs::create_dir_all(&locks_dir).expect("create locks dir");
    std::fs::write(
        locks_dir.join("codex.lock"),
        format!("{{\"pid\": {}}}", std::process::id()),
    )
    .expect("write lock");

    let mut dead_since = Some(Instant::now() - Duration::from_secs(999));
    let mut next_poll = Some(Instant::now() - Duration::from_secs(1));
    let mut last_activity = Instant::now() - Duration::from_secs(120);
    let stale_activity = last_activity;

    let terminate = should_terminate_for_idle(
        &mut last_activity,
        Duration::from_secs(1),
        Duration::from_secs(1),
        Some(tmp.path()),
        &mut dead_since,
        &mut next_poll,
    );

    assert!(!terminate, "alive process should not be terminated");
    assert!(dead_since.is_none(), "death timer should be cleared");
    assert!(
        last_activity > stale_activity,
        "last_activity should be reset to now (was {stale_activity:?}, now {last_activity:?})"
    );
}

#[tokio::test]
async fn test_idle_timeout_with_alive_process_does_not_kill() {
    // A silent process (sleep) with a live lock file for our own PID should
    // NOT be killed by idle timeout — liveness keeps it alive.
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    std::fs::create_dir_all(&locks_dir).expect("create locks dir");
    std::fs::write(
        locks_dir.join("codex.lock"),
        format!("{{\"pid\": {}}}", std::process::id()),
    )
    .expect("write lock");

    // "sleep 2" produces no output, but our PID in lock makes liveness=true.
    let mut cmd = Command::new("bash");
    cmd.args(["-c", "sleep 2"]);
    let child = spawn_tool(cmd, None).await.expect("spawn");
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(1),   // idle_timeout: fires quickly
        Duration::from_secs(600), // liveness_dead_timeout: very long
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        Some(&tmp.path().join("output.log")),
    )
    .await
    .expect("wait");

    assert_eq!(
        result.exit_code, 0,
        "process should exit naturally, not be killed (exit={})",
        result.exit_code
    );
}

#[tokio::test]
async fn test_idle_timeout_with_dead_process_kills_after_dead_timeout() {
    // A silent process without any liveness signals should be killed once
    // the liveness_dead_timeout expires.  Use output_spool=None so session_dir
    // is None, bypassing liveness checks — this tests the legacy kill path.
    let mut cmd = Command::new("bash");
    cmd.args(["-c", "sleep 30"]);
    let child = spawn_tool(cmd, None).await.expect("spawn");
    let start = Instant::now();
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(1), // idle_timeout
        Duration::from_secs(2), // liveness_dead_timeout
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None, // no session_dir → immediate kill after idle
    )
    .await
    .expect("wait");
    let elapsed = start.elapsed();

    assert_eq!(
        result.exit_code, 137,
        "process should be killed by idle timeout"
    );
    assert!(
        elapsed < Duration::from_secs(15),
        "should terminate near idle_timeout, elapsed={elapsed:?}"
    );
}

#[test]
fn test_is_working_reads_proc_stat() {
    // Our own process should be in R or S state.
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    std::fs::create_dir_all(&locks_dir).expect("create locks dir");
    std::fs::write(
        locks_dir.join("tool.lock"),
        format!("{{\"pid\": {}}}", std::process::id()),
    )
    .expect("write lock");

    assert!(
        ToolLiveness::is_working(tmp.path()),
        "is_working should return true for our own running process"
    );
}

#[test]
fn test_is_working_false_for_nonexistent_pid() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    std::fs::create_dir_all(&locks_dir).expect("create locks dir");
    // Use PID 1 (init) — we cannot send signal 0 to it without CAP_KILL,
    // and our lock file context won't match, so is_process_alive will fail
    // for most test environments. Use a clearly dead PID instead.
    std::fs::write(locks_dir.join("tool.lock"), "{\"pid\": 999999999}").expect("write lock");

    assert!(
        !ToolLiveness::is_working(tmp.path()),
        "is_working should return false for non-existent PID"
    );
}
