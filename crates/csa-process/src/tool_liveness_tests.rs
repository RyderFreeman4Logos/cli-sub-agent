use super::*;
#[cfg(target_os = "linux")]
fn daemon_pid_record(pid: u32) -> String {
    let metadata = read_process_metadata(pid).expect("process metadata");
    format!("{pid} {}\n", metadata.start_time_ticks)
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn wait_for_process_command_line_contains(pid: u32, expected: &str) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(100);
    let mut delay = Duration::from_millis(1);
    while std::time::Instant::now() < deadline {
        if read_process_command_line(pid).is_some_and(|cmdline| cmdline.contains(expected)) {
            return true;
        }
        std::thread::sleep(delay);
        delay = delay.saturating_mul(2).min(Duration::from_millis(16));
    }
    read_process_command_line(pid).is_some_and(|cmdline| cmdline.contains(expected))
}

#[test]
fn lock_file_is_recent_false_when_stale() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let lock_path = tmp.path().join("codex.lock");
    fs::write(&lock_path, "{\"pid\": 1}").expect("write lock");

    let stale_now = SystemTime::now() + Duration::from_secs(LOCK_FILE_STALE_SECS + 1);
    assert!(!lock_file_is_recent(&lock_path, stale_now));
}

#[test]
fn lock_file_is_recent_true_when_fresh() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let lock_path = tmp.path().join("codex.lock");
    fs::write(&lock_path, "{\"pid\": 1}").expect("write lock");

    assert!(lock_file_is_recent(&lock_path, SystemTime::now()));
}

#[cfg(unix)]
#[test]
fn is_working_returns_true_for_own_process() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    fs::create_dir_all(&locks_dir).expect("create locks dir");
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("sleep 60 # codex")
        .spawn()
        .unwrap();
    let pid = child.id();
    fs::write(
        locks_dir.join("codex.lock"),
        format!("{{\"pid\": {}}}", pid),
    )
    .expect("write lock");

    let working = ToolLiveness::is_working(tmp.path());
    child.kill().ok();
    child.wait().ok();
    assert!(working);
}

#[test]
fn is_working_returns_false_for_empty_session() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(!ToolLiveness::is_working(tmp.path()));
}

#[test]
fn is_pid_working_returns_true_for_self() {
    assert!(is_pid_working(std::process::id()));
}

#[cfg(unix)]
#[test]
fn find_session_pid_returns_own_pid() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    fs::create_dir_all(&locks_dir).expect("create locks dir");
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("sleep 60 # tool")
        .spawn()
        .unwrap();
    let pid = child.id();
    fs::write(locks_dir.join("tool.lock"), format!("{{\"pid\": {pid}}}")).expect("write lock");

    let found_pid = find_session_pid(tmp.path());
    child.kill().ok();
    child.wait().ok();
    assert_eq!(found_pid, Some(pid));
}

#[cfg(target_os = "linux")]
#[test]
fn daemon_pid_is_alive_detects_live_pid_without_context_match() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let pid = child.id();
    fs::write(tmp.path().join(DAEMON_PID_FILE), daemon_pid_record(pid)).expect("write daemon pid");

    assert!(
        ToolLiveness::daemon_pid_is_alive(tmp.path()),
        "raw daemon.pid should count as alive even when cmdline matching fails"
    );
    assert!(
        ToolLiveness::is_alive(tmp.path()),
        "coarse liveness should stay true while daemon.pid still exists"
    );
    assert!(
        !ToolLiveness::has_live_process(tmp.path()),
        "context-matched process detection should remain false for this fixture"
    );

    child.kill().ok();
    child.wait().ok();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn daemon_pid_is_alive_accepts_legacy_pid_with_session_id_context() {
    const SESSION_ID: &str = "01TESTSESSIONCONTEXT0000000001";

    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join(SESSION_ID);
    fs::create_dir_all(&session_dir).expect("create session dir");
    let mut child = std::process::Command::new("sh")
        .args(["-c", "sleep 60", "csa-daemon", SESSION_ID])
        .spawn()
        .unwrap();
    let pid = child.id();
    let cmdline_ready = wait_for_process_command_line_contains(pid, SESSION_ID);
    fs::write(session_dir.join(DAEMON_PID_FILE), format!("{pid}\n")).expect("write daemon pid");
    let daemon_pid_alive = ToolLiveness::daemon_pid_is_alive(&session_dir);

    child.kill().ok();
    child.wait().ok();

    assert!(
        cmdline_ready,
        "spawned daemon command line should expose session context"
    );
    assert!(daemon_pid_alive, "legacy bare daemon.pid should stay alive");
}

#[cfg(target_os = "linux")]
#[test]
fn daemon_pid_is_alive_rejects_start_time_mismatch() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let pid = child.id();
    fs::write(tmp.path().join(DAEMON_PID_FILE), format!("{pid} 0\n")).expect("write daemon pid");

    assert!(
        !ToolLiveness::daemon_pid_is_alive(tmp.path()),
        "start time mismatch must prevent unrelated PID reuse from blocking liveness"
    );

    child.kill().ok();
    child.wait().ok();
}

#[cfg(target_os = "linux")]
#[test]
fn daemon_pid_is_alive_rejects_zombie_without_live_process_group() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let pid = child.id();
    fs::write(tmp.path().join(DAEMON_PID_FILE), daemon_pid_record(pid)).expect("write daemon pid");

    child.kill().expect("kill child");
    for _ in 0..20 {
        if matches!(
            read_process_metadata(pid),
            Some(ProcessMetadata { state: 'Z', .. })
        ) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        !ToolLiveness::daemon_pid_is_alive(tmp.path()),
        "zombie daemon without live process-group members must not block finalization"
    );

    child.wait().ok();
}

#[test]
fn record_spool_bytes_written_persists_monotonic_counter() {
    let tmp = tempfile::tempdir().expect("tempdir");
    record_spool_bytes_written(tmp.path(), 1234);

    let snapshot = fs::read_to_string(tmp.path().join(SNAPSHOT_FILE)).expect("read snapshot");
    assert!(snapshot.contains("spool_bytes_written=1234"));
}

#[test]
fn probe_detects_spool_byte_growth_after_rotation() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(
        tmp.path().join(SNAPSHOT_FILE),
        "spool_bytes_written=4096\nobserved_spool_bytes_written=2048",
    )
    .expect("seed snapshot");

    let signals = ToolLiveness::probe(tmp.path());
    assert!(
        signals.output_growth,
        "monotonic spool bytes should count as progress"
    );

    let snapshot = fs::read_to_string(tmp.path().join(SNAPSHOT_FILE)).expect("read snapshot");
    assert!(snapshot.contains("observed_spool_bytes_written=4096"));
}

#[test]
fn probe_detects_fatal_error_marker_in_stderr_tail() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let padding = "x".repeat(8192);
    fs::write(
        tmp.path().join(STDERR_LOG_FILE),
        format!("{padding}\nbackend failed with HTTP 429 Too Many Requests\n"),
    )
    .expect("write stderr");

    let signals = ToolLiveness::probe(tmp.path());

    assert!(signals.fatal_error);
}

#[test]
fn probe_ignores_broad_http_markers_in_output_log_content() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(
        tmp.path().join(OUTPUT_LOG_FILE),
        "docs quote HTTP 404 Not Found and 500 Internal Server Error as examples\n",
    )
    .expect("write output");

    let signals = ToolLiveness::probe(tmp.path());

    assert!(!signals.fatal_error);
}

#[test]
fn fatal_error_signal_scopes_tmux_pane_to_tier1_markers() {
    let tmp = tempfile::tempdir().expect("tempdir");

    assert!(!has_fatal_error_signal_in_channels(
        tmp.path(),
        Some("agent quoted HTTP 429 while reading API docs")
    ));
    assert!(has_fatal_error_signal_in_channels(
        tmp.path(),
        Some("provider envelope: quota exceeded")
    ));
}

#[test]
fn probe_detects_broad_http_marker_in_stderr_tail() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(
        tmp.path().join(STDERR_LOG_FILE),
        "transport failed with HTTP 503\n",
    )
    .expect("write stderr");

    let signals = ToolLiveness::probe(tmp.path());

    assert!(signals.fatal_error);
}

#[test]
fn probe_detects_tier1_provider_marker_in_output_log() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(
        tmp.path().join(OUTPUT_LOG_FILE),
        "provider envelope: rate_limit_exceeded\n",
    )
    .expect("write output");

    let signals = ToolLiveness::probe(tmp.path());

    assert!(signals.fatal_error);
}

#[test]
fn probe_uses_custom_fatal_error_marker_sidecar() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_fatal_error_markers(tmp.path(), &["custom backend died".to_string()])
        .expect("write markers");
    fs::write(
        tmp.path().join(OUTPUT_LOG_FILE),
        "agent ui shows: custom backend died\n",
    )
    .expect("write output");

    let signals = ToolLiveness::probe(tmp.path());

    assert!(signals.fatal_error);
}

#[test]
fn probe_reuses_cached_custom_fatal_error_marker_regex() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_fatal_error_markers(tmp.path(), &["custom backend died".to_string()])
        .expect("write markers");
    fs::write(
        tmp.path().join(OUTPUT_LOG_FILE),
        "agent ui shows: custom backend died\n",
    )
    .expect("write output");

    assert!(ToolLiveness::probe(tmp.path()).fatal_error);

    write_fatal_error_markers(tmp.path(), &["different marker".to_string()])
        .expect("rewrite markers");

    assert!(ToolLiveness::probe(tmp.path()).fatal_error);
}

#[test]
fn empty_fatal_error_marker_sidecar_disables_defaults() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_fatal_error_markers(tmp.path(), &[]).expect("write markers");
    fs::write(
        tmp.path().join(STDERR_LOG_FILE),
        "backend failed with HTTP 500 Internal Server Error\n",
    )
    .expect("write stderr");

    let signals = ToolLiveness::probe(tmp.path());

    assert!(!signals.fatal_error);
}

#[test]
fn pid_matches_session_context_returns_true_for_recent_file_even_without_context_match() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pid = std::process::id();

    assert!(pid_matches_session_context(
        pid,
        Some("nonexistent"),
        tmp.path(),
        Some(true)
    ));
}

#[cfg(unix)]
#[test]
fn find_session_pid_ignores_reconcile_lock_in_parent_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    fs::create_dir_all(&locks_dir).expect("create locks dir");

    let reconcile_lock = tmp.path().join(".reconcile.lock");
    fs::write(&reconcile_lock, "{\"pid\": 12345}").expect("write lock");

    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("sleep 60 # tool")
        .spawn()
        .unwrap();
    let pid = child.id();
    fs::write(locks_dir.join("tool.lock"), format!("{{\"pid\": {pid}}}")).expect("write lock");

    let found_pid = find_session_pid(tmp.path());
    child.kill().ok();
    child.wait().ok();

    assert_eq!(found_pid, Some(pid));
}
