use super::*;
use crate::session_cmds_daemon::{WaitReconciliationOutcome, handle_session_wait_with_hooks};
use crate::test_env_lock::TEST_ENV_LOCK;
use chrono::Utc;
use csa_session::{
    SessionPhase, SessionResult, create_session, get_session_dir, load_result, load_session,
    save_result,
};
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
        |_| {},
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
        |_| {},
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
fn ensure_terminal_result_for_dead_active_session_is_noop_when_reconcile_lock_is_held() {
    let td = tempdir().expect("tempdir");
    let _env = SessionTestEnv::new(&td);
    let project = td.path();

    let session = create_session(project, Some("lock-held"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let (_session_dir, _lock) = acquire_reconcile_lock(project, &session_id, "unit-test")
        .unwrap()
        .expect("lock should be acquired for setup");

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    assert_eq!(reconciled, DeadActiveSessionReconciliation::NoChange);
    assert!(load_result(project, &session_id).unwrap().is_none());
    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(persisted.phase, SessionPhase::Active);
    assert_eq!(persisted.termination_reason, None);
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

#[cfg(unix)]
#[test]
fn ensure_terminal_result_for_dead_active_session_reconciles_even_with_reused_pid_in_daemon_pid() {
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
    fs::write(session_dir.join("daemon.pid"), format!("{pid}\n")).unwrap();

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();

    child.kill().ok();
    child.wait().ok();

    assert_eq!(
        reconciled,
        DeadActiveSessionReconciliation::SynthesizedFailure
    );
    let result = load_result(project, &session_id)
        .unwrap()
        .expect("result should be synthesized");
    assert_eq!(result.status, "failure");
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
