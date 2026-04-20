use super::*;
use crate::session_cmds_daemon::{WaitReconciliationOutcome, handle_session_wait_with_hooks};
use crate::test_env_lock::TEST_ENV_LOCK;
use chrono::Utc;
use csa_session::{
    PhaseEvent, SessionPhase, SessionResult, create_session, get_session_dir, load_result,
    load_session, save_result, save_session,
};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tokio::sync::OwnedMutexGuard;
use tracing_subscriber::fmt::MakeWriter;

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

#[derive(Clone, Default)]
struct SharedLogBuffer {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl SharedLogBuffer {
    fn contents(&self) -> String {
        String::from_utf8(self.bytes.lock().unwrap().clone()).unwrap()
    }
}

struct SharedLogWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = SharedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLogWriter {
            bytes: Arc::clone(&self.bytes),
        }
    }
}

impl std::io::Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bytes.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct SessionTestEnv {
    _env_lock: OwnedMutexGuard<()>,
    _home_guard: EnvVarGuard,
    _state_guard: EnvVarGuard,
}

impl SessionTestEnv {
    fn new(td: &tempfile::TempDir) -> Self {
        let env_lock = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let state_home = td.path().join("xdg-state");
        std::fs::create_dir_all(&state_home).expect("create state home");
        let home_guard = EnvVarGuard::set("HOME", td.path());
        let state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
        Self {
            _env_lock: env_lock,
            _home_guard: home_guard,
            _state_guard: state_guard,
        }
    }
}

fn make_result(status: &str, exit_code: i32) -> SessionResult {
    let now = Utc::now();
    SessionResult {
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
    use std::os::unix::ffi::OsStrExt;
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

#[cfg(unix)]
fn backdate_tree(path: &std::path::Path, seconds_ago: u64) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).expect("read_dir") {
            let entry = entry.expect("dir entry");
            backdate_tree(&entry.path(), seconds_ago);
        }
    }
    set_file_mtime_seconds_ago(path, seconds_ago);
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
fn ensure_terminal_result_for_dead_active_session_leaves_state_unchanged_on_transition_failure() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("transition-failure"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let state_path = session_dir.join("state.toml");
    let cooldown_path = session_dir.parent().unwrap().join("cooldown-marker.toml");
    let original_state = fs::read_to_string(&state_path).unwrap();

    let err = ensure_terminal_result_for_dead_active_session_impl(
        project,
        &session_id,
        "session list",
        &session_dir,
        SyntheticResultHooks {
            before_write: &noop_path,
            after_publish: &noop_path,
        },
        |session| {
            session.phase = SessionPhase::Retired;
        },
        &persist_session_state_atomically,
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("Failed to transition orphaned session to Retired phase"),
        "unexpected error: {err:#}"
    );
    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(persisted.phase, SessionPhase::Active);
    assert_eq!(persisted.termination_reason, None);
    assert_eq!(fs::read_to_string(&state_path).unwrap(), original_state);
    assert!(
        load_result(project, &session_id).unwrap().is_none(),
        "synthetic result should be removed after a transition failure"
    );
    assert!(
        !cooldown_path.exists(),
        "cooldown marker should not be written for a rolled-back reconciliation"
    );
}

#[test]
fn ensure_terminal_result_for_dead_active_session_removes_synthetic_result_on_save_failure() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("save-failure"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let state_path = session_dir.join("state.toml");
    let cooldown_path = session_dir.parent().unwrap().join("cooldown-marker.toml");
    let original_state = fs::read_to_string(&state_path).unwrap();
    let persist_fail = |_: &Path, _: &MetaSessionState| -> Result<()> { Err(anyhow!("boom")) };

    let err = ensure_terminal_result_for_dead_active_session_impl(
        project,
        &session_id,
        "session list",
        &session_dir,
        SyntheticResultHooks {
            before_write: &noop_path,
            after_publish: &noop_path,
        },
        |_| {},
        &persist_fail,
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("Failed to persist retired orphaned session state"),
        "unexpected error: {err:#}"
    );
    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(persisted.phase, SessionPhase::Active);
    assert_eq!(persisted.termination_reason, None);
    assert_eq!(fs::read_to_string(&state_path).unwrap(), original_state);
    assert!(
        load_result(project, &session_id).unwrap().is_none(),
        "synthetic result should be removed after a state persistence failure"
    );
    assert!(
        !cooldown_path.exists(),
        "cooldown marker should not be written when reconciliation state persistence fails"
    );
}

#[test]
fn rollback_does_not_delete_late_real_result() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("late-real-result-rollback"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let result_path = session_dir.join("result.toml");
    let late_result = SessionResult {
        summary: "late real terminal result".to_string(),
        ..make_result("success", 0)
    };
    let late_result_contents = toml::to_string_pretty(&late_result).unwrap();
    let persist_fail = |_: &Path, _: &MetaSessionState| -> Result<()> { Err(anyhow!("boom")) };

    let buffer = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(buffer.clone())
        .without_time()
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let after_publish = |path: &Path| {
        assert_eq!(path, result_path.as_path());
        fs::write(path, &late_result_contents).unwrap();
    };

    let err = ensure_terminal_result_for_dead_active_session_impl(
        project,
        &session_id,
        "session list",
        &session_dir,
        SyntheticResultHooks {
            before_write: &noop_path,
            after_publish: &after_publish,
        },
        |_| {},
        &persist_fail,
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("Failed to persist retired orphaned session state"),
        "unexpected error: {err:#}"
    );
    assert!(
        result_path.exists(),
        "late real result should remain on disk"
    );
    assert_eq!(
        fs::read_to_string(&result_path).unwrap(),
        late_result_contents
    );

    let persisted = load_result(project, &session_id).unwrap().unwrap();
    assert_eq!(persisted.status, "success");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.summary, "late real terminal result");

    let session_state = load_session(project, &session_id).unwrap();
    assert_eq!(session_state.phase, SessionPhase::Active);
    assert_eq!(session_state.termination_reason, None);

    let logs = buffer.contents();
    assert!(
        logs.contains("late real result.toml") && logs.contains("left it in place"),
        "expected late-real-result warning, got: {logs}"
    );
}

#[test]
fn retire_if_dead_with_result_leaves_state_unchanged_on_save_failure() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("retire-save-failure"), None, None).unwrap();
    let session_id = session.meta_session_id;
    save_result(project, &session_id, &make_result("success", 0)).unwrap();
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let state_path = session_dir.join("state.toml");
    let original_state = fs::read_to_string(&state_path).unwrap();
    let persist_fail = |_: &Path, _: &MetaSessionState| -> Result<()> { Err(anyhow!("boom")) };

    let err = retire_if_dead_with_result_impl(
        project,
        &session_id,
        "session list",
        &session_dir,
        reconcile_liveness_decision(&session_dir),
        &persist_fail,
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("Failed to persist retired session state"),
        "unexpected error: {err:#}"
    );
    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(persisted.phase, SessionPhase::Active);
    assert_eq!(persisted.termination_reason, None);
    assert_eq!(fs::read_to_string(&state_path).unwrap(), original_state);
    let result = load_result(project, &session_id).unwrap().unwrap();
    assert_eq!(result.status, "success");
    assert_eq!(result.exit_code, 0);
}

#[test]
fn ensure_terminal_result_for_dead_active_session_does_not_create_lock_file_when_no_change() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("no-change-no-lock"), None, None).unwrap();
    let session_id = session.meta_session_id;
    save_result(project, &session_id, &make_result("success", 0)).unwrap();
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let lock_path = session_dir.join(".reconcile.lock");

    assert!(
        !lock_path.exists(),
        "lock file should not exist before noop"
    );
    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    assert_eq!(reconciled, DeadActiveSessionReconciliation::NoChange);
    assert!(
        !lock_path.exists(),
        "noop reconciliation should not create a lock file"
    );
}

#[cfg(unix)]
#[test]
fn ensure_terminal_result_for_dead_active_session_skips_lock_on_read_only_dir_when_no_change() {
    use std::os::unix::fs::PermissionsExt;

    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("no-change-read-only"), None, None).unwrap();
    let session_id = session.meta_session_id;
    save_result(project, &session_id, &make_result("success", 0)).unwrap();
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let lock_path = session_dir.join(".reconcile.lock");
    let original_permissions = fs::metadata(&session_dir).unwrap().permissions();
    let read_only_permissions = std::fs::Permissions::from_mode(0o555);

    fs::set_permissions(&session_dir, read_only_permissions).unwrap();
    let outcome =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list");
    fs::set_permissions(&session_dir, original_permissions).unwrap();

    assert_eq!(outcome.unwrap(), DeadActiveSessionReconciliation::NoChange);
    assert!(
        !lock_path.exists(),
        "read-only noop reconciliation should not create a lock file"
    );
}

#[cfg(unix)]
#[test]
fn retire_if_dead_with_result_skips_lock_on_read_only_dir_when_no_change() {
    use std::os::unix::fs::PermissionsExt;

    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("retire-no-change-read-only"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let mut persisted = load_session(project, &session_id).unwrap();
    persisted.apply_phase_event(PhaseEvent::Retired).unwrap();
    save_session(&persisted).unwrap();
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let lock_path = session_dir.join(".reconcile.lock");
    let original_permissions = fs::metadata(&session_dir).unwrap().permissions();
    let read_only_permissions = std::fs::Permissions::from_mode(0o555);

    fs::set_permissions(&session_dir, read_only_permissions).unwrap();
    let outcome = retire_if_dead_with_result(project, &session_id, "session list");
    fs::set_permissions(&session_dir, original_permissions).unwrap();

    assert!(!outcome.unwrap());
    assert!(
        !lock_path.exists(),
        "read-only retire noop should not create a lock file"
    );
}

#[test]
fn persist_new_result_file_removes_partial_file_when_write_fails() {
    let td = tempdir().expect("tempdir");
    let result_path = td.path().join("result.toml");

    let err = persist_new_result_file_with_writer(
        &result_path,
        "status = \"failure\"\n",
        |_| {},
        |file, _contents| {
            file.write_all(b"partial")?;
            Err(std::io::Error::other("boom"))
        },
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("Failed to write or sync synthetic result"),
        "unexpected error: {err:#}"
    );
    assert!(
        !result_path.exists(),
        "partial synthetic result should be removed after write failure"
    );
}

#[cfg(unix)]
#[test]
fn persist_session_state_atomically_preserves_existing_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("state-permissions"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let state_path = session_dir.join("state.toml");
    fs::set_permissions(&state_path, std::fs::Permissions::from_mode(0o640)).unwrap();

    let mut persisted = load_session(project, &session_id).unwrap();
    persisted.termination_reason = Some("permission-check".to_string());
    persist_session_state_atomically(&session_dir, &persisted).unwrap();

    let mode = fs::metadata(&state_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
}

#[cfg(unix)]
#[test]
fn persist_new_result_file_defaults_to_private_permissions_for_new_files() {
    use std::os::unix::fs::PermissionsExt;

    let td = tempdir().expect("tempdir");
    let result_path = td.path().join("result.toml");

    let outcome = persist_new_result_file(
        &result_path,
        "status = \"failure\"\nexit_code = 1\nsummary = \"synthetic\"\n",
        |_| {},
    )
    .unwrap();

    assert_eq!(outcome, SyntheticResultPersistOutcome::Created);
    let mode = fs::metadata(&result_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn handle_session_wait_marks_late_real_result_completion_as_non_synthetic() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session =
        create_session(project, Some("wait-late-real-result"), None, Some("codex")).unwrap();
    let session_id = session.meta_session_id;
    let late_result = SessionResult {
        summary: "real terminal result".to_string(),
        ..make_result("success", 0)
    };
    let mut emitted_completion: Option<(String, String, i32, bool)> = None;

    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        5,
        |project_root, current_session_id, trigger| {
            let reconciled = ensure_terminal_result_for_dead_active_session_with_before_write(
                project_root,
                current_session_id,
                trigger,
                |_| {
                    save_result(project_root, current_session_id, &late_result)
                        .expect("persist late real result");
                },
            )?;
            Ok(WaitReconciliationOutcome {
                result_became_available: reconciled.result_became_available(),
                synthetic: reconciled.synthesized_failure(),
            })
        },
        |sid, status, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .unwrap();

    assert_eq!(exit_code, 0);
    assert_eq!(
        emitted_completion,
        Some((session_id, "success".to_string(), 0, false))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn ensure_terminal_result_for_dead_active_session_skips_synthesis_while_daemon_pid_is_alive() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("pid-reuse"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    // Use a live PID that definitely doesn't match our session context.
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let pid = child.id();
    fs::write(session_dir.join("daemon.pid"), daemon_pid_record(pid)).unwrap();
    assert!(
        !csa_process::ToolLiveness::has_live_process(&session_dir),
        "fixture should exercise raw daemon.pid fallback, not context-matched liveness"
    );
    assert!(csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir));

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    child.kill().ok();
    child.wait().ok();

    assert_eq!(reconciled, DeadActiveSessionReconciliation::NoChange);
    assert!(
        load_result(project, &session_id).unwrap().is_none(),
        "live daemon.pid must block synthetic failure"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn ensure_terminal_result_for_dead_active_session_reconciles_when_daemon_pid_is_dead() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("dead-daemon-pid"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let pid = child.id();
    fs::write(session_dir.join("daemon.pid"), daemon_pid_record(pid)).unwrap();
    child.kill().ok();
    child.wait().ok();
    assert!(!csa_process::ToolLiveness::daemon_pid_is_alive(
        &session_dir
    ));

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    assert_eq!(
        reconciled,
        DeadActiveSessionReconciliation::SynthesizedFailure
    );
    let result = load_result(project, &session_id)
        .unwrap()
        .expect("result should be synthesized");
    assert_eq!(result.status, "failure");
}

#[cfg(target_os = "linux")]
#[test]
fn ensure_terminal_result_for_dead_active_session_ignores_reused_daemon_pid_with_start_time_mismatch()
 {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("daemon-pid-reuse-guard"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let pid = child.id();
    fs::write(session_dir.join("daemon.pid"), format!("{pid} 0\n")).unwrap();
    assert!(
        !csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir),
        "start time mismatch must prevent unrelated PID reuse from blocking reconciliation"
    );

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    child.kill().ok();
    child.wait().ok();

    assert_eq!(
        reconciled,
        DeadActiveSessionReconciliation::SynthesizedFailure
    );
}

#[cfg(unix)]
#[test]
fn ensure_terminal_result_for_dead_active_session_reconciles_even_with_stale_lock_file() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("stale-lock"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    let locks_dir = session_dir.join("locks");
    fs::create_dir_all(&locks_dir).unwrap();
    let lock_path = locks_dir.join("tool.lock");

    // Use a live PID that definitely doesn't match our session context.
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let pid = child.id();
    fs::write(&lock_path, format!("{{\"pid\": {pid}}}")).unwrap();

    // Backdate the lock file to be stale (> 60s).
    #[cfg(unix)]
    backdate_tree(&lock_path, 120);

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    child.kill().ok();
    child.wait().ok();

    assert_eq!(
        reconciled,
        DeadActiveSessionReconciliation::SynthesizedFailure
    );
    assert!(load_result(project, &session_id).unwrap().is_some());
}

#[test]
fn extract_pid_handles_complex_json_correctly() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("complex-json"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    let locks_dir = session_dir.join("locks");
    fs::create_dir_all(&locks_dir).unwrap();
    let lock_path = locks_dir.join("tool.lock");

    // JSON that would confuse a simple substring search for "pid".
    // Use a live PID that doesn't match context.
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let pid = child.id();
    fs::write(
        &lock_path,
        format!("{{\"comment\": \"this is a fake pid: 123\", \"pid\": {pid}}}"),
    )
    .unwrap();

    // Age the lock file to be stale (>60s) so context check is required for liveness.
    let file = std::fs::File::options()
        .write(true)
        .open(&lock_path)
        .unwrap();
    let stale_time = std::time::SystemTime::now() - std::time::Duration::from_secs(70);
    file.set_times(std::fs::FileTimes::new().set_modified(stale_time))
        .unwrap();

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    child.kill().ok();
    child.wait().ok();

    // Should synthesize failure because PID doesn't match context.
    assert_eq!(
        reconciled,
        DeadActiveSessionReconciliation::SynthesizedFailure
    );
}
