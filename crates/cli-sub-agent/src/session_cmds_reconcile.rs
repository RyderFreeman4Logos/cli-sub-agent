use anyhow::{Context, Result, anyhow};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use csa_session::{
    MetaSessionState, SessionPhase, SessionResult, get_session_dir, load_result, load_session,
};

const RECONCILE_LOCK_NAME: &str = "reconcile";
type PersistSessionFn<'a> = dyn Fn(&Path, &MetaSessionState) -> Result<()> + 'a;

#[rustfmt::skip]
struct ReconcileLock { file: fs::File }

impl Drop for ReconcileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // SAFETY: `file` owns a valid fd; unlocking releases the advisory flock.
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeadActiveSessionReconciliation {
    NoChange,
    SynthesizedFailure,
    LateResultRetired,
}

#[rustfmt::skip]
impl DeadActiveSessionReconciliation {
    pub(crate) fn result_became_available(self) -> bool { matches!(self, Self::SynthesizedFailure | Self::LateResultRetired) }
    pub(crate) fn synthesized_failure(self) -> bool { matches!(self, Self::SynthesizedFailure) }
}

pub(crate) fn ensure_terminal_result_for_dead_active_session(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
) -> Result<DeadActiveSessionReconciliation> {
    let Some((session_dir, _lock)) = acquire_reconcile_lock(project_root, session_id, trigger)?
    else {
        return Ok(DeadActiveSessionReconciliation::NoChange);
    };
    ensure_terminal_result_for_dead_active_session_impl(
        project_root,
        session_id,
        trigger,
        &session_dir,
        |_| {},
        |_| {},
        &persist_session_state_atomically,
    )
}

#[cfg(test)]
pub(crate) fn ensure_terminal_result_for_dead_active_session_with_before_write<F>(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
    before_write: F,
) -> Result<DeadActiveSessionReconciliation>
where
    F: FnOnce(&Path),
{
    let Some((session_dir, _lock)) = acquire_reconcile_lock(project_root, session_id, trigger)?
    else {
        return Ok(DeadActiveSessionReconciliation::NoChange);
    };
    ensure_terminal_result_for_dead_active_session_impl(
        project_root,
        session_id,
        trigger,
        &session_dir,
        before_write,
        |_| {},
        &persist_session_state_atomically,
    )
}

fn ensure_terminal_result_for_dead_active_session_impl<F, B>(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
    session_dir: &Path,
    before_write: F,
    before_retire: B,
    persist_session: &PersistSessionFn<'_>,
) -> Result<DeadActiveSessionReconciliation>
where
    F: FnOnce(&Path),
    B: FnOnce(&mut MetaSessionState),
{
    let mut session = load_session(project_root, session_id)?;
    if !matches!(session.phase, SessionPhase::Active) {
        return Ok(DeadActiveSessionReconciliation::NoChange);
    }
    if session_has_live_tool_process(session_dir) {
        return Ok(DeadActiveSessionReconciliation::NoChange);
    }
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    match load_result(project_root, session_id) {
        Ok(Some(_)) => return Ok(DeadActiveSessionReconciliation::NoChange),
        Ok(None) => {}
        Err(err) if result_path.is_file() => {
            warn!(
                session_id = %session_id,
                trigger = %trigger,
                reconciliation_reason = "late_result_write_unreadable",
                result_path = %result_path.display(),
                error = %err,
                "Result file appeared during dead-session reconciliation; preserving late writer and skipping synthetic fallback"
            );
            return Ok(DeadActiveSessionReconciliation::NoChange);
        }
        Err(err) => return Err(err),
    }

    let now = chrono::Utc::now();
    let tool_name = session
        .tools
        .iter()
        .max_by_key(|(_, state)| state.updated_at)
        .map(|(tool, _)| tool.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let artifacts =
        crate::pipeline_post_exec::collect_fallback_result_artifacts(project_root, session_id);
    let output_log_mtime = format_optional_file_mtime(&session_dir.join("output.log"));
    let summary_prefix = format!(
        "synthetic failure by {trigger}: process dead, result.toml missing (reconciliation_reason=true_missing_result, output_log_mtime={})",
        output_log_mtime.as_deref().unwrap_or("missing")
    );
    let fallback = SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary: crate::pipeline_post_exec::build_fallback_result_summary(
            session_dir,
            &summary_prefix,
        ),
        tool: tool_name,
        started_at: std::cmp::min(session.last_accessed, now),
        completed_at: now,
        events_count: 0,
        artifacts,
        peak_memory_mb: None,
    };
    let result_contents = toml::to_string_pretty(&fallback)
        .map_err(|err| anyhow!("Failed to serialize synthetic result for {session_id}: {err}"))?;
    match persist_new_result_file(&result_path, &result_contents, before_write)? {
        SyntheticResultPersistOutcome::AlreadyExists => {
            let retired = retire_if_dead_with_result_impl(
                project_root,
                session_id,
                trigger,
                session_dir,
                persist_session,
            )?;
            info!(
                session_id = %session_id,
                trigger = %trigger,
                reconciliation_reason = "late_result_write",
                result_path = %result_path.display(),
                result_mtime = %format_optional_file_mtime(&result_path).unwrap_or_else(|| "unknown".to_string()),
                "Late result.toml write won during dead-session reconciliation"
            );
            return Ok(if retired {
                DeadActiveSessionReconciliation::LateResultRetired
            } else {
                DeadActiveSessionReconciliation::NoChange
            });
        }
        SyntheticResultPersistOutcome::Created => {}
    }

    before_retire(&mut session);
    if let Err(err) = session.apply_phase_event(csa_session::PhaseEvent::Retired) {
        warn!(
            session_id = %session_id,
            trigger = %trigger,
            reconciliation_reason = "true_missing_result",
            error = %err,
            "Failed to transition orphaned session to Retired phase during reconciliation; removing synthetic result and leaving session state unchanged"
        );
        remove_result_file(&result_path).map_err(|cleanup_err| {
            anyhow!(
                "Failed to transition orphaned session to Retired phase during reconciliation for {session_id}: {err}; additionally failed to remove synthetic result {}: {cleanup_err}",
                result_path.display()
            )
        })?;
        return Ok(DeadActiveSessionReconciliation::NoChange);
    }
    session.termination_reason = Some("orphaned_process".to_string());
    if let Err(err) = persist_session(session_dir, &session) {
        warn!(
            session_id = %session_id,
            trigger = %trigger,
            reconciliation_reason = "true_missing_result",
            error = %err,
            "Failed to persist retired orphaned session state during reconciliation; removing synthetic result and leaving session state unchanged"
        );
        remove_result_file(&result_path).map_err(|cleanup_err| {
            anyhow!(
                "Failed to persist retired orphaned session state for {session_id}: {err}; additionally failed to remove synthetic result {}: {cleanup_err}",
                result_path.display()
            )
        })?;
        return Err(anyhow!(
            "Failed to persist retired orphaned session state for {session_id}: {err}"
        ));
    }
    csa_session::write_cooldown_marker_from_session_dir(
        session_dir,
        session_id,
        fallback.completed_at,
    );
    warn!(
        session_id = %session_id,
        trigger = %trigger,
        reconciliation_reason = "true_missing_result",
        result_path = %result_path.display(),
        output_log_mtime = %output_log_mtime.unwrap_or_else(|| "missing".to_string()),
        "Recovered orphaned session with synthetic result"
    );
    Ok(DeadActiveSessionReconciliation::SynthesizedFailure)
}

pub(crate) fn retire_if_dead_with_result(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
) -> Result<bool> {
    let Some((session_dir, _lock)) = acquire_reconcile_lock(project_root, session_id, trigger)?
    else {
        return Ok(false);
    };
    retire_if_dead_with_result_impl(
        project_root,
        session_id,
        trigger,
        &session_dir,
        &persist_session_state_atomically,
    )
}

fn retire_if_dead_with_result_impl(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
    session_dir: &Path,
    persist_session: &PersistSessionFn<'_>,
) -> Result<bool> {
    let mut session = load_session(project_root, session_id)?;
    if !matches!(session.phase, SessionPhase::Active) {
        return Ok(false);
    }
    if session_has_live_tool_process(session_dir)
        || load_result(project_root, session_id)?.is_none()
    {
        return Ok(false);
    }
    if session
        .apply_phase_event(csa_session::PhaseEvent::Retired)
        .is_err()
    {
        return Ok(false);
    }
    session
        .termination_reason
        .get_or_insert_with(|| "completed".to_string());
    persist_session(session_dir, &session)
        .with_context(|| format!("Failed to persist retired session state for {session_id}"))?;
    info!(
        session_id = %session_id,
        trigger = %trigger,
        "Retired dead Active session with result"
    );
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[rustfmt::skip]
enum SyntheticResultPersistOutcome { Created, AlreadyExists }

fn acquire_reconcile_lock(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
) -> Result<Option<(PathBuf, ReconcileLock)>> {
    let session_dir = get_session_dir(project_root, session_id)?;
    let lock_path = session_dir.join(".reconcile.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| {
            format!(
                "Failed to open reconciliation lock: {}",
                lock_path.display()
            )
        })?;

    #[cfg(unix)]
    {
        // SAFETY: `file` owns a valid fd and `LOCK_EX|LOCK_NB` is a non-destructive advisory lock.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret == 0 {
            return Ok(Some((session_dir, ReconcileLock { file })));
        }

        let errno = std::io::Error::last_os_error().raw_os_error();
        if errno == Some(libc::EWOULDBLOCK) || errno == Some(libc::EAGAIN) {
            info!(
                session_id = %session_id,
                trigger = %trigger,
                "Skipping reconciliation because another process already holds the reconcile lock"
            );
            return Ok(None);
        }

        Err(anyhow!(
            "Failed to acquire reconciliation lock for {session_id}: {}",
            std::io::Error::last_os_error()
        ))
    }

    #[cfg(not(unix))]
    {
        let _ = trigger;
        Ok(Some((session_dir, ReconcileLock { file })))
    }
}

fn session_has_live_tool_process(session_dir: &Path) -> bool {
    if read_session_daemon_pid(session_dir).is_some_and(is_process_alive) {
        return true;
    }

    let locks_dir = session_dir.join("locks");
    let Ok(entries) = fs::read_dir(&locks_dir) else {
        return false;
    };

    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "lock") {
            return false;
        }
        if path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem == RECONCILE_LOCK_NAME)
        {
            return false;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            return false;
        };
        extract_lock_pid(&content).is_some_and(is_process_alive)
    })
}

#[rustfmt::skip]
fn read_session_daemon_pid(session_dir: &Path) -> Option<u32> { fs::read_to_string(session_dir.join("daemon.pid")).ok()?.trim().parse::<u32>().ok() }

fn extract_lock_pid(lock_content: &str) -> Option<u32> {
    let pid_key_pos = lock_content.find("\"pid\"")?;
    let tail = &lock_content[pid_key_pos..];
    let colon_pos = tail.find(':')?;
    let number = tail[colon_pos + 1..]
        .chars()
        .skip_while(|ch| ch.is_ascii_whitespace())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    number.parse::<u32>().ok()
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `kill(pid, 0)` performs an existence probe without sending a signal.
        let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if ret == 0 {
            return true;
        }
        let errno = std::io::Error::last_os_error().raw_os_error();
        errno == Some(libc::EPERM)
    }

    #[cfg(not(unix))]
    {
        std::path::Path::new(&format!("/proc/{pid}/stat")).exists()
    }
}

fn persist_session_state_atomically(session_dir: &Path, session: &MetaSessionState) -> Result<()> {
    let state_path = session_dir.join("state.toml");
    let contents = toml::to_string_pretty(session).context("Failed to serialize session state")?;
    let mut temp_file = tempfile::NamedTempFile::new_in(session_dir).with_context(|| {
        format!(
            "Failed to create temporary state file in {}",
            session_dir.display()
        )
    })?;
    temp_file.write_all(contents.as_bytes()).with_context(|| {
        format!(
            "Failed to write temporary state file: {}",
            state_path.display()
        )
    })?;
    temp_file.as_file_mut().sync_all().with_context(|| {
        format!(
            "Failed to sync temporary state file: {}",
            state_path.display()
        )
    })?;
    temp_file.persist(&state_path).map_err(|err| {
        anyhow!(
            "Failed to persist state file {}: {}",
            state_path.display(),
            err.error
        )
    })?;
    Ok(())
}

fn remove_result_file(result_path: &Path) -> std::io::Result<()> {
    match fs::remove_file(result_path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn persist_new_result_file<F>(
    result_path: &Path,
    contents: &str,
    before_write: F,
) -> Result<SyntheticResultPersistOutcome>
where
    F: FnOnce(&Path),
{
    persist_new_result_file_with_writer(result_path, contents, before_write, |file, contents| {
        file.write_all(contents.as_bytes())?;
        file.sync_all()
    })
}

fn persist_new_result_file_with_writer<F, W>(
    result_path: &Path,
    contents: &str,
    before_write: F,
    write_contents: W,
) -> Result<SyntheticResultPersistOutcome>
where
    F: FnOnce(&Path),
    W: FnOnce(&mut fs::File, &str) -> std::io::Result<()>,
{
    before_write(result_path);
    let result_dir = result_path.parent().ok_or_else(|| {
        anyhow!(
            "Synthetic result path has no parent: {}",
            result_path.display()
        )
    })?;
    let mut temp_file = tempfile::NamedTempFile::new_in(result_dir).with_context(|| {
        format!(
            "Failed to create temporary synthetic result in {}",
            result_dir.display()
        )
    })?;
    if let Err(err) = write_contents(temp_file.as_file_mut(), contents) {
        return Err(anyhow!(
            "Failed to write or sync synthetic result for {}: {err}",
            result_path.display()
        ));
    }
    match fs::hard_link(temp_file.path(), result_path) {
        Ok(()) => Ok(SyntheticResultPersistOutcome::Created),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            Ok(SyntheticResultPersistOutcome::AlreadyExists)
        }
        Err(err) => Err(anyhow!(
            "Failed to publish synthetic result for {}: {err}",
            result_path.display()
        )),
    }
}

fn format_optional_file_mtime(path: &Path) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let modified = chrono::DateTime::<chrono::Utc>::from(modified);
    Some(modified.to_rfc3339())
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn ensure_terminal_result_for_dead_active_session_leaves_state_unchanged_on_transition_failure()
    {
        let td = tempdir().expect("tempdir");
        let _env = SessionTestEnv::new(&td);
        let project = td.path();

        let session = create_session(project, Some("transition-failure"), None, None).unwrap();
        let session_id = session.meta_session_id;
        let session_dir = get_session_dir(project, &session_id).unwrap();
        let state_path = session_dir.join("state.toml");
        let cooldown_path = session_dir.parent().unwrap().join("cooldown-marker.toml");
        let original_state = fs::read_to_string(&state_path).unwrap();

        let reconciled = ensure_terminal_result_for_dead_active_session_impl(
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
        .unwrap();

        assert_eq!(reconciled, DeadActiveSessionReconciliation::NoChange);
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
                emitted_completion =
                    Some((sid.to_string(), status.to_string(), exit_code, synthetic));
            },
        )
        .unwrap();

        assert_eq!(exit_code, 0);
        assert_eq!(
            emitted_completion,
            Some((session_id, "success".to_string(), 0, false))
        );
    }
}
