use anyhow::{Context, Result, anyhow};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;
use tracing::{info, warn};

use csa_process::ToolLiveness;
use csa_session::{
    MetaSessionState, SessionPhase, SessionResult, get_session_dir, load_result, load_session,
};

type PersistSessionFn<'a> = dyn Fn(&Path, &MetaSessionState) -> Result<()> + 'a;

struct SyntheticResultHooks<'a> {
    before_write: &'a dyn Fn(&Path),
    after_publish: &'a dyn Fn(&Path),
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

fn with_reconcile_lock<R>(session_dir: &Path, body: impl FnOnce() -> Result<R>) -> Result<R> {
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

    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock
        .write()
        .map_err(|e| anyhow::Error::from(e).context("Failed to acquire reconciliation lock"))?;
    body()
}

fn noop_path(_: &Path) {}

pub(crate) fn ensure_terminal_result_for_dead_active_session(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
) -> Result<DeadActiveSessionReconciliation> {
    let session_dir = get_session_dir(project_root, session_id)?;
    if !dead_active_session_needs_terminal_result(project_root, session_id, trigger, &session_dir)?
    {
        return Ok(DeadActiveSessionReconciliation::NoChange);
    }
    with_reconcile_lock(&session_dir, || {
        ensure_terminal_result_for_dead_active_session_impl(
            project_root,
            session_id,
            trigger,
            &session_dir,
            SyntheticResultHooks {
                before_write: &noop_path,
                after_publish: &noop_path,
            },
            |_| {},
            &persist_session_state_atomically,
        )
    })
}

#[cfg(test)]
pub(crate) fn ensure_terminal_result_for_dead_active_session_with_before_write<F>(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
    before_write: F,
) -> Result<DeadActiveSessionReconciliation>
where
    F: Fn(&Path),
{
    let session_dir = get_session_dir(project_root, session_id)?;
    if !dead_active_session_needs_terminal_result(project_root, session_id, trigger, &session_dir)?
    {
        return Ok(DeadActiveSessionReconciliation::NoChange);
    }
    let after_publish = noop_path;
    with_reconcile_lock(&session_dir, || {
        ensure_terminal_result_for_dead_active_session_impl(
            project_root,
            session_id,
            trigger,
            &session_dir,
            SyntheticResultHooks {
                before_write: &before_write,
                after_publish: &after_publish,
            },
            |_| {},
            &persist_session_state_atomically,
        )
    })
}

fn dead_active_session_needs_terminal_result(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
    session_dir: &Path,
) -> Result<bool> {
    let session = load_session(project_root, session_id)?;
    if !matches!(session.phase, SessionPhase::Active) {
        return Ok(false);
    }
    if ToolLiveness::has_live_process(session_dir) {
        return Ok(false);
    }
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    match load_result(project_root, session_id) {
        Ok(Some(_)) => Ok(false),
        Ok(None) => Ok(true),
        Err(err) if result_path.is_file() => {
            warn!(
                session_id = %session_id,
                trigger = %trigger,
                reconciliation_reason = "late_result_write_unreadable",
                result_path = %result_path.display(),
                error = %err,
                "Result file appeared during dead-session reconciliation; preserving late writer and skipping synthetic fallback"
            );
            Ok(false)
        }
        Err(err) => Err(err),
    }
}

fn ensure_terminal_result_for_dead_active_session_impl<B>(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
    session_dir: &Path,
    hooks: SyntheticResultHooks<'_>,
    before_retire: B,
    persist_session: &PersistSessionFn<'_>,
) -> Result<DeadActiveSessionReconciliation>
where
    B: FnOnce(&mut MetaSessionState),
{
    let SyntheticResultHooks {
        before_write,
        after_publish,
    } = hooks;
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

    after_publish(&result_path);
    before_retire(&mut session);
    if let Err(err) = session.apply_phase_event(csa_session::PhaseEvent::Retired) {
        warn!(
            session_id = %session_id,
            trigger = %trigger,
            reconciliation_reason = "true_missing_result",
            error = %err,
            "Failed to transition orphaned session to Retired phase during reconciliation; removing synthetic result and leaving session state unchanged"
        );
        remove_synthetic_result_if_unchanged(&result_path, result_contents.as_bytes()).map_err(
            |cleanup_err| {
            anyhow!(
                "Failed to transition orphaned session to Retired phase during reconciliation for {session_id}: {err}; additionally failed to remove synthetic result {}: {cleanup_err}",
                result_path.display()
            )
        },
        )?;
        return Err(anyhow!(
            "Failed to transition orphaned session to Retired phase during reconciliation for {session_id}: {err}"
        ));
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
        remove_synthetic_result_if_unchanged(&result_path, result_contents.as_bytes()).map_err(
            |cleanup_err| {
            anyhow!(
                "Failed to persist retired orphaned session state for {session_id}: {err}; additionally failed to remove synthetic result {}: {cleanup_err}",
                result_path.display()
            )
        },
        )?;
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
    let session_dir = get_session_dir(project_root, session_id)?;
    if !dead_session_with_result_needs_retire(project_root, session_id, &session_dir)? {
        return Ok(false);
    }
    with_reconcile_lock(&session_dir, || {
        retire_if_dead_with_result_impl(
            project_root,
            session_id,
            trigger,
            &session_dir,
            &persist_session_state_atomically,
        )
    })
}

fn dead_session_with_result_needs_retire(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
) -> Result<bool> {
    let session = load_session(project_root, session_id)?;
    if !matches!(session.phase, SessionPhase::Active) {
        return Ok(false);
    }
    if ToolLiveness::has_live_process(session_dir) {
        return Ok(false);
    }
    Ok(load_result(project_root, session_id)?.is_some())
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
    preserve_existing_permissions_if_present(temp_file.as_file_mut(), &state_path, "state file")?;
    temp_file.persist(&state_path).map_err(|err| {
        anyhow!(
            "Failed to persist state file {}: {}",
            state_path.display(),
            err.error
        )
    })?;
    Ok(())
}

fn remove_synthetic_result_if_unchanged(
    result_path: &Path,
    expected_contents: &[u8],
) -> std::io::Result<()> {
    match fs::read(result_path) {
        Ok(current_contents) if current_contents == expected_contents => {
            match fs::remove_file(result_path) {
                Ok(()) => {
                    warn!(
                        result_path = %result_path.display(),
                        rollback_cleanup = "removed_synthetic_result",
                        "Rollback removed synthetic result.toml after reconciliation failure"
                    );
                    Ok(())
                }
                Err(err) if err.kind() == ErrorKind::NotFound => {
                    warn!(
                        result_path = %result_path.display(),
                        rollback_cleanup = "result_missing_after_match",
                        "Rollback synthetic result.toml was already absent after reconciliation failure"
                    );
                    Ok(())
                }
                Err(err) => {
                    warn!(
                        result_path = %result_path.display(),
                        rollback_cleanup = "remove_failed",
                        error = %err,
                        "Rollback failed to remove matching synthetic result.toml after reconciliation failure"
                    );
                    Err(err)
                }
            }
        }
        Ok(_) => {
            warn!(
                result_path = %result_path.display(),
                rollback_cleanup = "late_real_result_preserved",
                "Rollback detected late real result.toml and left it in place"
            );
            Ok(())
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            warn!(
                result_path = %result_path.display(),
                rollback_cleanup = "result_missing",
                "Rollback found no result.toml to clean up after reconciliation failure"
            );
            Ok(())
        }
        Err(err) => {
            warn!(
                result_path = %result_path.display(),
                rollback_cleanup = "read_failed",
                error = %err,
                "Rollback failed to read result.toml for content-aware cleanup after reconciliation failure"
            );
            Ok(())
        }
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
    preserve_existing_permissions_if_present(
        temp_file.as_file_mut(),
        result_path,
        "synthetic result",
    )?;
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

fn preserve_existing_permissions_if_present(
    temp_file: &mut fs::File,
    target_path: &Path,
    file_kind: &str,
) -> Result<()> {
    let permissions = match fs::metadata(target_path) {
        Ok(metadata) => Some(metadata.permissions()),
        Err(err) if err.kind() == ErrorKind::NotFound => None,
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "Failed to read {file_kind} metadata before preserving permissions: {}",
                    target_path.display()
                )
            });
        }
    };
    if let Some(permissions) = permissions {
        temp_file.set_permissions(permissions).with_context(|| {
            format!(
                "Failed to preserve existing permissions for {file_kind}: {}",
                target_path.display()
            )
        })?;
    }
    Ok(())
}

fn format_optional_file_mtime(path: &Path) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let modified = chrono::DateTime::<chrono::Utc>::from(modified);
    Some(modified.to_rfc3339())
}

#[cfg(test)]
#[path = "session_cmds_reconcile_tests.rs"]
mod tests;
