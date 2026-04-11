use super::*;

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
    let c = a; // Clone (Copy-forwarded)
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
fn test_should_terminate_resets_last_activity_on_progress_signal() {
    // Progress signals should reset both the death timer and idle timer.
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

    assert!(!terminate, "progress should prevent termination");
    assert!(dead_since.is_none(), "death timer should be cleared");
    assert!(
        last_activity > stale_activity,
        "last_activity should be reset to now (was {stale_activity:?}, now {last_activity:?})"
    );
}

#[cfg(unix)]
fn set_file_mtime_seconds_ago(path: &std::path::Path, seconds_ago: u64) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX_EPOCH");
    let target_sec = now.as_secs().saturating_sub(seconds_ago) as libc::time_t;
    let times = [
        libc::timespec {
            tv_sec: target_sec,
            tv_nsec: 0,
        },
        libc::timespec {
            tv_sec: target_sec,
            tv_nsec: 0,
        },
    ];

    let c_path = CString::new(path.as_os_str().as_bytes()).expect("path contains interior NUL");
    // SAFETY: `c_path` is a valid C string, `times` points to two valid
    // timespec entries for atime/mtime, and flags=0 targets the path itself.
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
    assert_eq!(
        rc,
        0,
        "utimensat failed for {}: {:?}",
        path.display(),
        std::io::Error::last_os_error()
    );
}

#[tokio::test]
async fn test_idle_timeout_with_alive_process_does_not_kill() {
    // A silent process (sleep) with a live lock file for our own PID should
    // NOT be killed when liveness_dead_timeout is much longer than runtime.
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    std::fs::create_dir_all(&locks_dir).expect("create locks dir");
    std::fs::write(
        locks_dir.join("codex.lock"),
        format!("{{\"pid\": {}}}", std::process::id()),
    )
    .expect("write lock");

    // "sleep 2" produces no output and exits before liveness_dead_timeout.
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
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("wait");

    assert_eq!(
        result.exit_code, 0,
        "process should exit naturally, not be killed (exit={})",
        result.exit_code
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_idle_timeout_with_live_pid_but_no_progress_kills_after_dead_timeout() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    std::fs::create_dir_all(&locks_dir).expect("create locks dir");
    let lock_path = locks_dir.join("codex.lock");
    std::fs::write(&lock_path, format!("{{\"pid\": {}}}", std::process::id())).expect("write lock");
    // Make metadata stale so lock-file timestamp is not misclassified as progress.
    set_file_mtime_seconds_ago(&lock_path, 120);

    let output_path = tmp.path().join("output.log");
    std::fs::write(&output_path, "").expect("seed output");
    set_file_mtime_seconds_ago(&output_path, 120);

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
        Some(&output_path), // enables liveness mode
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("wait");
    let elapsed = start.elapsed();

    assert_eq!(
        result.exit_code, 137,
        "silent live PID without progress should be treated as hang"
    );
    assert!(
        elapsed < Duration::from_secs(15),
        "should terminate after idle+liveness_dead window, elapsed={elapsed:?}"
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
        SpawnOptions::default(),
        None,
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
    // Use a spawned process with 'tool' in cmdline to satisfy context check.
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("sleep 60 # tool")
        .spawn()
        .unwrap();
    let pid = child.id();
    std::fs::write(locks_dir.join("tool.lock"), format!("{{\"pid\": {}}}", pid))
        .expect("write lock");

    let working = ToolLiveness::is_working(tmp.path());
    child.kill().ok();
    child.wait().ok();
    assert!(
        working,
        "is_working should return true for a running process with correct context"
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

// --- drain_if_over_high_water tests ---

#[test]
fn test_drain_if_over_high_water_no_op_below_threshold() {
    let mut buf = "x".repeat(output_helpers::TAIL_BUFFER_HIGH_WATER - 1);
    let original_len = buf.len();
    output_helpers::drain_if_over_high_water(&mut buf);
    assert_eq!(buf.len(), original_len, "should not drain below high-water");
}

#[test]
fn test_drain_if_over_high_water_drains_at_threshold() {
    let mut buf = "x".repeat(output_helpers::TAIL_BUFFER_HIGH_WATER + 1);
    output_helpers::drain_if_over_high_water(&mut buf);
    assert!(
        buf.len() <= output_helpers::TAIL_BUFFER_MAX_BYTES + 1,
        "should drain to ~TAIL_BUFFER_MAX_BYTES, got {}",
        buf.len()
    );
}

#[test]
fn test_drain_preserves_tail_content() {
    // Build a large buffer with a known tail marker
    let padding = "a".repeat(output_helpers::TAIL_BUFFER_HIGH_WATER);
    let marker = "TAIL_MARKER_UNIQUE";
    let mut buf = format!("{padding}{marker}");
    output_helpers::drain_if_over_high_water(&mut buf);
    assert!(
        buf.ends_with(marker),
        "tail marker should be preserved after drain"
    );
}

#[test]
fn test_drain_handles_multibyte_utf8() {
    // Fill with multi-byte chars (emoji = 4 bytes each)
    let emoji = "🔥";
    let count = (output_helpers::TAIL_BUFFER_HIGH_WATER / emoji.len()) + 10;
    let mut buf: String = emoji.repeat(count);
    assert!(buf.len() > output_helpers::TAIL_BUFFER_HIGH_WATER);
    output_helpers::drain_if_over_high_water(&mut buf);
    // Must be valid UTF-8 (no panic) and bounded
    assert!(buf.len() <= output_helpers::TAIL_BUFFER_MAX_BYTES + emoji.len());
    // Every char should still be a valid emoji
    assert!(buf.chars().all(|c| c == '🔥'));
}

#[test]
fn test_output_string_bounded_at_high_water() {
    // Simulate accumulating 10 MiB of output line by line
    let mut output = String::new();
    let line = format!("{}\n", "x".repeat(999)); // ~1KB per line
    let target_bytes = 10 * 1024 * 1024; // 10 MiB
    let mut written = 0usize;
    while written < target_bytes {
        output.push_str(&line);
        written += line.len();
        output_helpers::drain_if_over_high_water(&mut output);
    }
    assert!(
        output.len() <= output_helpers::TAIL_BUFFER_HIGH_WATER + line.len(),
        "output should be bounded: got {} bytes, limit ~{} bytes",
        output.len(),
        output_helpers::TAIL_BUFFER_HIGH_WATER + line.len()
    );
}

#[test]
fn test_stderr_string_bounded_at_high_water() {
    // Same test for stderr path
    let mut stderr_output = String::new();
    let line = format!("warning: {}\n", "w".repeat(200));
    let target_bytes = 10 * 1024 * 1024;
    let mut written = 0usize;
    while written < target_bytes {
        stderr_output.push_str(&line);
        written += line.len();
        output_helpers::drain_if_over_high_water(&mut stderr_output);
    }
    assert!(
        stderr_output.len() <= output_helpers::TAIL_BUFFER_HIGH_WATER + line.len(),
        "stderr should be bounded: got {} bytes",
        stderr_output.len()
    );
}

#[test]
fn test_spool_rotator_rotates_and_writes_truncation_sentinel() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output_path = tmp.path().join("output.log");
    let rotated_path = output_path.with_extension("log.rotated");
    let mut rotator = SpoolRotator::open(&output_path, 16, true).expect("open rotator");

    rotator.write(b"1234567890").expect("write first chunk");
    rotator.write(b"abcdefghij").expect("write second chunk");
    rotator.flush().expect("flush rotator");
    drop(rotator);

    let rotated = std::fs::read_to_string(&rotated_path).expect("read rotated file");
    assert_eq!(rotated, "1234567890");

    let current = std::fs::read_to_string(&output_path).expect("read current file");
    assert!(
        current.starts_with("[CSA:TRUNCATED bytes_written=10 rotated_at="),
        "rotation should prepend truncation sentinel, got: {current}"
    );
    assert!(
        current.ends_with("abcdefghij"),
        "new output should continue in fresh output.log"
    );
}

// --- should_compress_output tests ---

#[test]
fn test_compress_small_output_passes_through() {
    let small = "a".repeat(100);
    assert!(matches!(
        should_compress_output(&small, 8192),
        CompressDecision::PassThrough
    ));
}

#[test]
fn test_compress_large_output_triggers_compression() {
    let large = "x".repeat(10_000);
    match should_compress_output(&large, 8192) {
        CompressDecision::Compress {
            original_bytes,
            replacement,
        } => {
            assert_eq!(original_bytes, 10_000);
            assert!(replacement.contains("10000 bytes"));
        }
        CompressDecision::PassThrough => panic!("expected Compress"),
    }
}

#[test]
fn test_compress_preserves_csa_section_markers() {
    let content = format!(
        "{}<!-- CSA:SECTION:summary -->{}",
        "x".repeat(5000),
        "x".repeat(5000)
    );
    assert!(matches!(
        should_compress_output(&content, 8192),
        CompressDecision::PassThrough
    ));
}

#[test]
fn test_compress_preserves_return_packet() {
    let content = format!("{}ReturnPacket{}", "x".repeat(5000), "x".repeat(5000));
    assert!(matches!(
        should_compress_output(&content, 8192),
        CompressDecision::PassThrough
    ));
}

#[test]
fn test_compress_at_exact_threshold_passes_through() {
    let exact = "y".repeat(8192);
    assert!(matches!(
        should_compress_output(&exact, 8192),
        CompressDecision::PassThrough
    ));
}
