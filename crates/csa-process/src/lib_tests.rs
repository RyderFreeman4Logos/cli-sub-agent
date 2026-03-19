use super::*;

#[path = "lib_tests_tail.rs"]
mod tail_tests;

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
        SpawnOptions::default(),
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
        SpawnOptions::default(),
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
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("Failed to wait");

    assert_eq!(result.exit_code, 0);
    assert!(result.output.contains("tick"));
}

#[tokio::test]
async fn test_wait_and_capture_repeated_workspace_boundary_errors_fails_fast() {
    let mut cmd = Command::new("bash");
    cmd.args([
        "-c",
        r#"for _ in 1 2 3 4; do
echo "Error executing tool read_file: Path not in workspace: /tmp/foo resolves outside the allowed workspace directories";
sleep 0.2;
done;
sleep 30"#,
    ]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let start = Instant::now();
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(30),
        Duration::from_secs(30),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("Failed to wait");
    let elapsed = start.elapsed();

    assert_eq!(
        result.exit_code, 125,
        "expected fail-fast exit for repeated workspace boundary errors"
    );
    assert!(
        result.summary.contains("workspace boundary timeout"),
        "summary should explain boundary fail-fast, got: {}",
        result.summary
    );
    assert!(
        result.output.contains("Path not in workspace"),
        "captured output should retain boundary errors"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "process should terminate quickly on repeated boundary errors, elapsed={elapsed:?}"
    );
}

#[tokio::test]
async fn test_wait_and_capture_single_workspace_boundary_error_does_not_fail_fast() {
    let mut cmd = Command::new("bash");
    cmd.args([
        "-c",
        r#"echo "Error executing tool read_file: Path not in workspace: /tmp/foo"; echo done"#,
    ]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("Failed to wait");

    assert_eq!(
        result.exit_code, 0,
        "single boundary error should not be terminal"
    );
    assert!(result.output.contains("done"));
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
        SpawnOptions::default(),
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
    let start = Instant::now();
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(1),
        Duration::from_secs(1),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        Some(&tmp.path().join("output.log")),
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("wait");
    let elapsed = start.elapsed();

    assert!(
        elapsed >= Duration::from_millis(1800),
        "watchdog should wait through liveness grace period, elapsed={elapsed:?}"
    );
    assert!(
        matches!(result.exit_code, 0 | 137),
        "process may exit naturally or hit kill race at boundary, exit_code={}",
        result.exit_code
    );
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
    std::fs::write(tmp.path().join("output.log"), "progress").expect("write output");
    std::fs::write(
        tmp.path().join(".liveness.snapshot"),
        "spool_bytes_written=8\nobserved_spool_bytes_written=0",
    )
    .expect("seed snapshot");

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
        "progress signal should reset death timer"
    );
    assert!(
        last_activity > before,
        "progress signal should reset idle timer"
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
fn test_failure_summary_skips_opaque_stdout_and_prefers_stderr() {
    let summary = failure_summary(
        "An unexpected critical error occurred:[object Object]\n",
        "model not found: gemini-pro\n",
        1,
    );
    assert_eq!(summary, "model not found: gemini-pro");
}

#[test]
fn test_failure_summary_normalizes_opaque_line_with_context() {
    let summary = failure_summary(
        "An unexpected critical error occurred:[object Object]\n",
        "",
        1,
    );
    assert_eq!(
        summary,
        "An unexpected critical error occurred (opaque error payload)"
    );
}

#[test]
fn test_failure_summary_opaque_marker_only_falls_back_to_explicit_message() {
    let summary = failure_summary("[object Object]\n", "", 17);
    assert_eq!(summary, "opaque tool error payload; exit code 17");
}

#[test]
fn test_failure_summary_skips_structural_stdout_and_prefers_stderr() {
    let summary = failure_summary("{\n { }\n}\n", "model not found: gemini-pro\n", 1);
    assert_eq!(summary, "model not found: gemini-pro");
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
async fn test_failed_command_sanitizes_opaque_stderr_payload() {
    let mut cmd = Command::new("bash");
    cmd.args([
        "-c",
        "echo 'An unexpected critical error occurred:[object Object]' >&2; exit 1",
    ]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture(child, StreamMode::BufferOnly)
        .await
        .expect("Failed to wait");

    assert_eq!(result.exit_code, 1);
    assert!(
        !result.stderr_output.contains("(opaque error payload)"),
        "stderr should not contain opaque payload marker in final output"
    );
    assert!(
        !result.stderr_output.contains("[object Object]"),
        "stderr should not expose raw opaque marker"
    );
    assert!(
        result
            .stderr_output
            .contains("resolved failure detail: exit code 1"),
        "stderr should include actionable fallback detail"
    );
}

#[tokio::test]
async fn test_failed_command_sanitizes_opaque_stderr_payload_case_insensitive() {
    let mut cmd = Command::new("bash");
    cmd.args([
        "-c",
        "echo 'An unexpected critical error occurred:[OBJECT OBJECT]' >&2; exit 1",
    ]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture(child, StreamMode::BufferOnly)
        .await
        .expect("Failed to wait");

    assert_eq!(result.exit_code, 1);
    assert!(
        !result.stderr_output.contains("(opaque error payload)"),
        "stderr should not contain opaque payload marker in final output"
    );
    assert!(
        !result
            .stderr_output
            .to_ascii_lowercase()
            .contains("[object object]"),
        "stderr should not expose raw opaque marker in any case"
    );
    assert!(
        result
            .stderr_output
            .contains("resolved failure detail: exit code 1"),
        "stderr should include actionable fallback detail"
    );
}

#[tokio::test]
async fn test_failed_command_sanitizes_opaque_stdout_payload() {
    let mut cmd = Command::new("bash");
    cmd.args(["-c", "echo '[object Object]'; exit 1"]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture(child, StreamMode::BufferOnly)
        .await
        .expect("Failed to wait");

    assert_eq!(result.exit_code, 1);
    assert!(
        result.output.contains("(opaque error payload)"),
        "stdout should contain normalized opaque marker"
    );
    assert!(
        !result.output.contains("[object Object]"),
        "stdout should not expose raw opaque marker"
    );
}

#[tokio::test]
async fn test_output_and_stderr_spools_sanitize_only_appended_segment() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output_log = tmp.path().join("output.log");
    let stderr_log = tmp.path().join("stderr.log");
    std::fs::write(&output_log, "legacy stdout line\n").expect("seed output log");
    std::fs::write(&stderr_log, "legacy stderr line\n").expect("seed stderr log");

    let mut cmd = Command::new("bash");
    cmd.args([
        "-c",
        "echo 'code: 404'; echo 'An unexpected critical error occurred:[object Object]' >&2; exit 1",
    ]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
        Duration::from_secs(DEFAULT_LIVENESS_DEAD_SECS),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        Some(&output_log),
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("Failed to wait");

    assert_eq!(result.exit_code, 1);
    assert!(
        !result
            .stderr_output
            .to_ascii_lowercase()
            .contains("[object object]"),
        "captured stderr should not include raw opaque marker"
    );
    assert!(
        result
            .stderr_output
            .contains("resolved failure detail: code: 404"),
        "captured stderr should append actionable failure detail"
    );
    assert_eq!(result.summary, "code: 404");
    assert!(
        !result
            .output
            .to_ascii_lowercase()
            .contains("[object object]"),
        "captured stdout should not include raw opaque marker"
    );

    let output_spool = std::fs::read_to_string(&output_log).expect("read output spool");
    assert!(
        output_spool.starts_with("legacy stdout line\n"),
        "existing output spool prefix should be preserved"
    );
    assert!(
        output_spool.contains("code: 404"),
        "output spool should preserve actionable stdout payload"
    );
    assert!(
        !output_spool
            .to_ascii_lowercase()
            .contains("[object object]"),
        "output spool should not keep raw opaque marker"
    );

    let stderr_spool = std::fs::read_to_string(&stderr_log).expect("read stderr spool");
    assert!(
        stderr_spool.starts_with("legacy stderr line\n"),
        "existing stderr spool prefix should be preserved"
    );
    assert!(
        !stderr_spool.contains("(opaque error payload)"),
        "stderr spool should not keep opaque payload marker"
    );
    assert!(
        stderr_spool.contains("resolved failure detail: code: 404"),
        "stderr spool should append actionable failure detail"
    );
    assert!(
        !stderr_spool
            .to_ascii_lowercase()
            .contains("[object object]"),
        "stderr spool should not keep raw opaque marker"
    );
    let stderr_tail = stderr_spool
        .lines()
        .last()
        .expect("stderr spool should have at least one line");
    assert_eq!(stderr_tail, "resolved failure detail: code: 404");
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
