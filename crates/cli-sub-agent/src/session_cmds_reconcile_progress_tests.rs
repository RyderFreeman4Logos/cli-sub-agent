use super::*;
use crate::test_env_lock::TEST_ENV_LOCK;
use chrono::Utc;
use csa_session::{create_session, get_session_dir, load_result, load_session, save_result};
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use tempfile::tempdir;

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe {
            match self.original.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

struct SessionTestEnv {
    _env_lock: std::sync::MutexGuard<'static, ()>,
    _home_guard: EnvVarGuard,
    _state_guard: EnvVarGuard,
}

impl SessionTestEnv {
    fn new(td: &tempfile::TempDir) -> Self {
        let env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
        let state_home = td.path().join("xdg-state");
        fs::create_dir_all(&state_home).expect("create state home");
        let home_guard = EnvVarGuard::set("HOME", td.path());
        let state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
        Self {
            _env_lock: env_lock,
            _home_guard: home_guard,
            _state_guard: state_guard,
        }
    }
}

fn write_liveness_snapshot(
    session_dir: &std::path::Path,
    lines: impl IntoIterator<Item = impl AsRef<str>>,
) {
    let contents = lines
        .into_iter()
        .map(|line| line.as_ref().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        session_dir.join(".liveness.snapshot"),
        format!("{contents}\n"),
    )
    .expect("write liveness snapshot");
}

#[test]
fn reconciler_does_not_clobber_runtime_liveness_snapshot() {
    let td = tempdir().expect("tempdir");
    let session_dir = td.path();

    fs::write(session_dir.join("output.log"), "fresh output bytes\n").expect("write output log");
    write_liveness_snapshot(&session_dir, ["spool_bytes_written=19"]);

    let snapshot_path = session_dir.join(".liveness.snapshot");
    let before = fs::read(&snapshot_path).expect("read runtime snapshot before reconcile");
    let before_mtime = fs::metadata(&snapshot_path)
        .expect("stat runtime snapshot before reconcile")
        .modified()
        .expect("mtime before reconcile");

    let decision = reconcile_liveness_decision(session_dir);

    let after = fs::read(&snapshot_path).expect("read runtime snapshot after reconcile");
    let after_mtime = fs::metadata(&snapshot_path)
        .expect("stat runtime snapshot after reconcile")
        .modified()
        .expect("mtime after reconcile");

    assert!(
        decision.blocks_synthesis,
        "fresh runtime liveness signal should still block synthesis"
    );
    assert_eq!(
        after, before,
        "reconcile must not rewrite the runtime-owned .liveness.snapshot"
    );
    assert_eq!(
        after_mtime, before_mtime,
        "reconcile must not update the runtime snapshot mtime"
    );
}

#[test]
fn missing_current_sizes_do_not_count_as_reconcile_progress() {
    let td = tempdir().expect("tempdir");
    let session_dir = td.path();

    write_liveness_snapshot(
        &session_dir,
        [
            "observed_spool_bytes_written=19",
            "acp_events_size=7",
            "stderr_log_size=3",
        ],
    );

    let decision = reconcile_liveness_decision(session_dir);

    assert!(
        !decision.blocks_synthesis,
        "missing current files should not be misclassified as fresh progress"
    );
    assert_eq!(decision.reason, "no_pid_no_progress");
}

fn make_result(status: &str, exit_code: i32) -> csa_session::SessionResult {
    let now = Utc::now();
    csa_session::SessionResult {
        status: status.to_string(),
        exit_code,
        summary: "summary".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        manager_fields: Default::default(),
    }
}

#[cfg(unix)]
fn set_file_mtime_seconds_ago(path: &std::path::Path, seconds_ago: u64) {
    use std::ffi::CString;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("system clock before unix epoch");
    let target = now.saturating_sub(std::time::Duration::from_secs(seconds_ago));
    let tv_sec = target.as_secs() as libc::time_t;
    let tv_nsec = target.subsec_nanos() as libc::c_long;
    let times = [
        libc::timespec { tv_sec, tv_nsec },
        libc::timespec { tv_sec, tv_nsec },
    ];
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("path contains NUL");
    // SAFETY: `utimensat` receives a valid C path pointer and valid timespec array.
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
    assert_eq!(rc, 0, "utimensat failed for {}", path.display());
}

#[cfg(target_os = "linux")]
fn read_process_start_time_ticks(pid: u32) -> u64 {
    let stat_path = format!("/proc/{pid}/stat");
    let content = fs::read_to_string(stat_path).expect("read /proc stat");
    let close_paren = content.rfind(')').expect("stat comm terminator");
    let after_comm = &content[close_paren + 1..];
    let mut parts = after_comm.split_whitespace();
    parts.next().expect("state");
    parts.next().expect("ppid");
    parts.next().expect("pgrp");
    for _ in 0..16 {
        parts.next().expect("intermediate stat field");
    }
    parts
        .next()
        .expect("starttime")
        .parse::<u64>()
        .expect("starttime parse")
}

#[cfg(target_os = "linux")]
fn daemon_pid_record(pid: u32) -> String {
    format!("{pid} {}\n", read_process_start_time_ticks(pid))
}

#[test]
fn fresh_stderr_activity_blocks_synthesis() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("fresh-stderr-activity"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    fs::write(session_dir.join("stderr.log"), "new stderr bytes\n").unwrap();
    write_liveness_snapshot(&session_dir, ["stderr_log_size=1"]);

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    assert_eq!(reconciled, DeadActiveSessionReconciliation::NoChange);
    assert!(
        load_result(project, &session_id).unwrap().is_none(),
        "stderr growth should block synthetic failure"
    );
}

#[test]
fn first_reconcile_with_fresh_output_no_prior_snapshot_blocks_synthesis() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session =
        create_session(project, Some("first-reconcile-fresh-output"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    fs::write(session_dir.join("output.log"), "fresh output bytes\n").unwrap();
    write_liveness_snapshot(&session_dir, ["spool_bytes_written=19"]);

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    assert_eq!(reconciled, DeadActiveSessionReconciliation::NoChange);
    assert!(
        load_result(project, &session_id).unwrap().is_none(),
        "fresh output before any observed snapshot should block synthetic failure"
    );
}

#[test]
fn first_reconcile_with_fresh_output_and_missing_snapshot_still_blocks_synthesis() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(
        project,
        Some("first-reconcile-fresh-output-missing-snapshot"),
        None,
        None,
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    fs::write(session_dir.join("output.log"), "fresh output bytes\n").unwrap();

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    assert_eq!(reconciled, DeadActiveSessionReconciliation::NoChange);
    assert!(
        load_result(project, &session_id).unwrap().is_none(),
        "fresh output with no snapshot file should still block synthetic failure"
    );
}

#[test]
fn first_reconcile_with_fresh_acp_events_no_prior_snapshot_blocks_synthesis() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(
        project,
        Some("first-reconcile-fresh-acp-events"),
        None,
        None,
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    fs::create_dir_all(session_dir.join("output")).unwrap();
    fs::write(
        session_dir.join("output/acp-events.jsonl"),
        "{\"type\":\"event\"}\n",
    )
    .unwrap();

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    assert_eq!(reconciled, DeadActiveSessionReconciliation::NoChange);
    assert!(
        load_result(project, &session_id).unwrap().is_none(),
        "fresh ACP event output before any observed snapshot should block synthetic failure"
    );
}

#[test]
fn retirement_with_fresh_output_and_existing_result_blocks_retirement() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(
        project,
        Some("retirement-fresh-output-existing-result"),
        None,
        None,
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    save_result(project, &session_id, &make_result("success", 0)).unwrap();
    fs::write(session_dir.join("output.log"), "fresh output bytes\n").unwrap();
    write_liveness_snapshot(&session_dir, ["spool_bytes_written=19"]);

    let retired = retire_if_dead_with_result(project, &session_id, "session list").unwrap();

    assert!(
        !retired,
        "fresh output should block dead-session retirement even when result.toml exists"
    );
    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(persisted.phase, csa_session::SessionPhase::Active);
    assert_eq!(persisted.termination_reason, None);
}

#[test]
fn concurrent_reconcile_with_synced_snapshot_still_blocks_synthesis() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(
        project,
        Some("concurrent-reconcile-synced-snapshot"),
        None,
        None,
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    fs::write(session_dir.join("output.log"), "fresh output bytes\n").unwrap();
    write_liveness_snapshot(
        &session_dir,
        ["spool_bytes_written=19", "observed_spool_bytes_written=19"],
    );

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    assert_eq!(reconciled, DeadActiveSessionReconciliation::NoChange);
    assert!(
        load_result(project, &session_id).unwrap().is_none(),
        "fresh output with a synced snapshot should still block synthetic failure"
    );
}

#[cfg(unix)]
#[test]
fn stale_output_after_grace_still_allows_synthesis() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("stale-output-after-grace"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    let output_path = session_dir.join("output.log");
    fs::write(&output_path, "old output bytes\n").unwrap();
    set_file_mtime_seconds_ago(&output_path, 31);
    write_liveness_snapshot(
        &session_dir,
        ["spool_bytes_written=16", "observed_spool_bytes_written=16"],
    );

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    assert_eq!(
        reconciled,
        DeadActiveSessionReconciliation::SynthesizedFailure
    );
    assert!(
        load_result(project, &session_id).unwrap().is_some(),
        "stale output without live pid/progress should still synthesize a terminal result"
    );
}

#[cfg(unix)]
#[test]
fn retirement_with_stale_output_and_existing_result_still_retires() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(
        project,
        Some("retirement-stale-output-existing-result"),
        None,
        None,
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    save_result(project, &session_id, &make_result("success", 0)).unwrap();
    let output_path = session_dir.join("output.log");
    fs::write(&output_path, "old output bytes\n").unwrap();
    set_file_mtime_seconds_ago(&output_path, 31);
    write_liveness_snapshot(
        &session_dir,
        ["spool_bytes_written=16", "observed_spool_bytes_written=16"],
    );

    let retired = retire_if_dead_with_result(project, &session_id, "session list").unwrap();

    assert!(
        retired,
        "stale output should still allow legitimate retirement"
    );
    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(persisted.phase, csa_session::SessionPhase::Retired);
    assert_eq!(persisted.termination_reason.as_deref(), Some("completed"));
}

#[cfg(target_os = "linux")]
#[test]
fn live_daemon_pid_still_blocks_synthesis() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("live-daemon-pid-blocks"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let pid = child.id();
    fs::write(session_dir.join("daemon.pid"), daemon_pid_record(pid)).unwrap();
    assert!(csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir));

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    child.kill().ok();
    child.wait().ok();

    assert_eq!(reconciled, DeadActiveSessionReconciliation::NoChange);
    assert!(
        load_result(project, &session_id).unwrap().is_none(),
        "live daemon.pid must continue blocking synthetic failure"
    );
}
