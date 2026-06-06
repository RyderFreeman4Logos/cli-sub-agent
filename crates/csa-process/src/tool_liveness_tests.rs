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

#[cfg(unix)]
struct PathEnvGuard {
    previous: Option<std::ffi::OsString>,
}

#[cfg(unix)]
impl PathEnvGuard {
    fn prepend(path: &std::path::Path) -> Self {
        let previous = std::env::var_os("PATH");
        let mut paths = vec![path.to_path_buf()];
        if let Some(existing) = previous.as_ref() {
            paths.extend(std::env::split_paths(existing));
        }
        let joined = std::env::join_paths(paths).expect("join PATH entries");
        // SAFETY: this unit test is Unix-only and restores PATH on drop; csa-process
        // tests do not concurrently mutate PATH.
        unsafe { std::env::set_var("PATH", joined) };
        Self { previous }
    }
}

#[cfg(unix)]
impl Drop for PathEnvGuard {
    fn drop(&mut self) {
        // SAFETY: restores the process PATH value captured by this test guard.
        unsafe {
            match self.previous.as_ref() {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
    }
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
fn is_alive_read_only_does_not_persist_liveness_snapshot() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let acp_path = tmp.path().join("output").join("acp-events.jsonl");
    fs::create_dir_all(acp_path.parent().expect("acp path parent")).expect("create output dir");
    fs::write(&acp_path, "{}\n").expect("write acp events");

    assert!(ToolLiveness::is_alive_read_only(tmp.path()));
    assert!(
        !tmp.path().join(SNAPSHOT_FILE).exists(),
        "read-only liveness checks must not write the watchdog snapshot"
    );

    assert!(ToolLiveness::probe(tmp.path()).has_any_signal());
    assert!(
        tmp.path().join(SNAPSHOT_FILE).exists(),
        "the normal execution/watchdog probe must still persist the snapshot"
    );
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
fn active_scope_ignores_provider_marker_from_failed_over_backend() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(
        tmp.path().join(STDERR_LOG_FILE),
        "gemini failed: HTTP 429 Too Many Requests reason: 'QUOTA_EXHAUSTED'\n",
    )
    .expect("write stale stderr");

    reset_liveness_scope(tmp.path(), "codex").expect("reset active scope");

    let signals = ToolLiveness::probe(tmp.path());

    assert!(
        !signals.fatal_error,
        "provider marker before active scope must not trip fallback backend"
    );
}

#[test]
fn active_scope_detects_provider_marker_from_current_backend() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(
        tmp.path().join(STDERR_LOG_FILE),
        "gemini failed: HTTP 429 Too Many Requests reason: 'QUOTA_EXHAUSTED'\n",
    )
    .expect("write stale stderr");
    reset_liveness_scope(tmp.path(), "codex").expect("reset active scope");
    fs::OpenOptions::new()
        .append(true)
        .open(tmp.path().join(STDERR_LOG_FILE))
        .expect("open stderr")
        .write_all(b"codex failed: HTTP 500 Internal Server Error\n")
        .expect("append active stderr");

    let signals = ToolLiveness::probe(tmp.path());

    assert!(
        signals.fatal_error,
        "provider marker after active scope must still trip current backend"
    );
}

#[test]
fn reset_liveness_scope_seeds_progress_baseline_for_new_backend() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join(OUTPUT_LOG_FILE), "old output\n").expect("write output");
    fs::write(
        tmp.path().join(SNAPSHOT_FILE),
        "spool_bytes_written=999\nobserved_spool_bytes_written=999\nstderr_log_size=999\n",
    )
    .expect("write stale snapshot");

    reset_liveness_scope(tmp.path(), "codex").expect("reset active scope");
    let current_len = fs::metadata(tmp.path().join(OUTPUT_LOG_FILE))
        .expect("output metadata")
        .len();
    record_spool_bytes_written(tmp.path(), current_len + 8);

    let signals = ToolLiveness::probe(tmp.path());

    assert!(
        signals.output_growth,
        "first active backend output after reset should count as fresh progress"
    );
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
fn sidecar_scopes_broad_http_markers_to_stderr_only() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_fatal_error_markers(
        tmp.path(),
        &[
            "HTTP 404".to_string(),
            "500 Internal Server Error".to_string(),
            "rate_limit_exceeded".to_string(),
        ],
    )
    .expect("write markers");
    fs::write(
        tmp.path().join(OUTPUT_LOG_FILE),
        "docs quote HTTP 404 Not Found and 500 Internal Server Error as examples\n",
    )
    .expect("write output");

    let signals = ToolLiveness::probe(tmp.path());

    assert!(!signals.fatal_error);
}

#[test]
fn sidecar_detects_broad_http_marker_in_stderr_tail() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_fatal_error_markers(
        tmp.path(),
        &[
            "HTTP 404".to_string(),
            "500 Internal Server Error".to_string(),
            "rate_limit_exceeded".to_string(),
        ],
    )
    .expect("write markers");
    // The exact configured code ("HTTP 404") on stderr fast-fails.
    fs::write(
        tmp.path().join(STDERR_LOG_FILE),
        "transport failed with HTTP 404\n",
    )
    .expect("write stderr");

    let signals = ToolLiveness::probe(tmp.path());

    assert!(signals.fatal_error);
}

#[test]
fn sidecar_broad_http_marker_ignores_unconfigured_status_code() {
    // #1652 round-5: broad HTTP markers must match the SPECIFIC configured code, not any
    // 3-digit code. A configured "HTTP 404" must NOT fast-fail on a non-fatal / unrelated
    // code such as "HTTP 200" or "HTTP 301" appearing on stderr.
    let tmp = tempfile::tempdir().expect("tempdir");
    write_fatal_error_markers(
        tmp.path(),
        &[
            "HTTP 404".to_string(),
            "500 Internal Server Error".to_string(),
            "rate_limit_exceeded".to_string(),
        ],
    )
    .expect("write markers");
    fs::write(
        tmp.path().join(STDERR_LOG_FILE),
        "server replied HTTP 200 OK then redirected HTTP 301\n",
    )
    .expect("write stderr");

    let signals = ToolLiveness::probe(tmp.path());

    assert!(!signals.fatal_error);
}

#[test]
fn sidecar_tier1_markers_scoped_to_stderr() {
    // #1830: tier-1 provider markers are scanned ONLY on the stderr transport stream.
    // The same marker present only in model/assistant output (`output.log`) must not
    // trip the scanner; on stderr it still fast-fails.
    let tmp = tempfile::tempdir().expect("tempdir");
    write_fatal_error_markers(
        tmp.path(),
        &[
            "HTTP 404".to_string(),
            "500 Internal Server Error".to_string(),
            "rate_limit_exceeded".to_string(),
        ],
    )
    .expect("write markers");
    fs::write(
        tmp.path().join(OUTPUT_LOG_FILE),
        "provider envelope: rate_limit_exceeded\n",
    )
    .expect("write output");

    assert!(
        !ToolLiveness::probe(tmp.path()).fatal_error,
        "tier-1 marker in model output must not trip the scanner"
    );

    fs::write(
        tmp.path().join(STDERR_LOG_FILE),
        "provider envelope: rate_limit_exceeded\n",
    )
    .expect("write stderr");

    assert!(
        ToolLiveness::probe(tmp.path()).fatal_error,
        "tier-1 marker on the stderr transport stream must still fast-fail"
    );
}

#[test]
fn matches_custom_markers_with_non_word_edges() {
    let markers = ["[ERROR]", "fatal"].map(String::from);
    let regex = build_fatal_error_regex(&markers).expect("regex");

    assert!(regex.is_match("[ERROR] unavailable"));
    assert!(regex.is_match("fatal error"));
    assert!(!regex.is_match("nonfatal error"));
}

#[test]
fn fatal_error_signal_excludes_model_output_channel() {
    // #1830: marker-like strings present only in model/assistant output (`output.log`,
    // and likewise the no-longer-scanned tmux pane) must NOT register as a provider
    // error — that text is model-authored, not a genuine transport failure.
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(
        tmp.path().join(OUTPUT_LOG_FILE),
        "agent quoted HTTP 429 while reading API docs; provider envelope: quota exceeded\n",
    )
    .expect("write output");

    assert!(!ToolLiveness::probe(tmp.path()).fatal_error);
}

#[cfg(unix)]
#[test]
fn provider_error_scan_does_not_spawn_tmux_capture_pane() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().join("01KTMUXPROBE0000000000000000");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&session_dir).expect("create session dir");
    fs::create_dir_all(&bin_dir).expect("create bin dir");

    let fake_tmux = bin_dir.join("tmux");
    fs::write(
        &fake_tmux,
        "#!/bin/sh\nprintf invoked >> \"$(dirname \"$0\")/tmux-called\"\nprintf 'provider envelope: quota exceeded\\n'\n",
    )
    .expect("write fake tmux");
    let mut permissions = fs::metadata(&fake_tmux)
        .expect("fake tmux metadata")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    fs::set_permissions(&fake_tmux, permissions).expect("chmod fake tmux");

    let _path_guard = PathEnvGuard::prepend(&bin_dir);

    assert!(
        !ToolLiveness::probe(&session_dir).fatal_error,
        "provider-error scan must not read marker text from tmux capture-pane"
    );
    assert!(
        !bin_dir.join("tmux-called").exists(),
        "#1670 regression: non-tmux liveness probes must not fork tmux"
    );

    fs::write(
        session_dir.join(STDERR_LOG_FILE),
        "provider envelope: quota exceeded\n",
    )
    .expect("write stderr");
    assert!(
        ToolLiveness::probe(&session_dir).fatal_error,
        "stderr transport markers must still fast-fail"
    );
    assert!(
        !bin_dir.join("tmux-called").exists(),
        "stderr-only provider scan should not need tmux even when detecting a marker"
    );
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
fn probe_ignores_tier1_provider_marker_in_output_log() {
    // #1830: a tier-1 provider marker that appears only in model/assistant output is
    // not a genuine transport error and must not trip the fatal-error scanner.
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(
        tmp.path().join(OUTPUT_LOG_FILE),
        "provider envelope: rate_limit_exceeded\n",
    )
    .expect("write output");

    let signals = ToolLiveness::probe(tmp.path());

    assert!(!signals.fatal_error);
}

#[test]
fn probe_uses_custom_fatal_error_marker_sidecar() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_fatal_error_markers(tmp.path(), &["custom backend died".to_string()])
        .expect("write markers");
    // Custom sidecar markers are matched on the stderr transport stream (#1830).
    fs::write(
        tmp.path().join(STDERR_LOG_FILE),
        "transport error: custom backend died\n",
    )
    .expect("write stderr");

    let signals = ToolLiveness::probe(tmp.path());

    assert!(signals.fatal_error);
}

#[test]
fn probe_reloads_custom_fatal_error_marker_sidecar() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_fatal_error_markers(tmp.path(), &["custom backend died".to_string()])
        .expect("write markers");
    fs::write(
        tmp.path().join(STDERR_LOG_FILE),
        "transport error: custom backend died\n",
    )
    .expect("write stderr");

    assert!(ToolLiveness::probe(tmp.path()).fatal_error);

    write_fatal_error_markers(tmp.path(), &["different marker".to_string()])
        .expect("rewrite markers");

    assert!(!ToolLiveness::probe(tmp.path()).fatal_error);

    fs::write(
        tmp.path().join(STDERR_LOG_FILE),
        "transport error: different marker\n",
    )
    .expect("rewrite stderr");

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
