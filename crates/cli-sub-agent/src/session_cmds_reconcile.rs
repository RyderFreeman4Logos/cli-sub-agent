use anyhow::{Context, Result, anyhow};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use csa_process::ToolLiveness;
use csa_session::{
    MetaSessionState, SessionPhase, SessionResult, get_session_dir, load_result, load_session,
};

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
    if ToolLiveness::has_live_process(session_dir) {
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
    if ToolLiveness::has_live_process(session_dir)
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
#[path = "session_cmds_reconcile_tests.rs"]
mod tests;
