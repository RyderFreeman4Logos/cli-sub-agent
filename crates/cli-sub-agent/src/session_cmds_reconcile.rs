use anyhow::{Context, Result, anyhow};
use csa_session::{
    MetaSessionState, SessionPhase, SessionResult, get_session_dir, load_result, load_session,
};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;
use tracing::{debug, info, warn};

#[cfg(test)]
use crate::session_result_publish::publish_result_file_if_absent_with_writer;
use crate::session_result_publish::{
    ResultFilePublishOutcome as SyntheticResultPersistOutcome,
    preserve_existing_permissions_if_present, publish_result_file_if_absent,
};
#[path = "session_cmds_reconcile_cleanup.rs"]
mod reconcile_cleanup;
#[path = "session_cmds_reconcile_diagnostics.rs"]
mod reconcile_diagnostics;
#[path = "session_cmds_reconcile_fix_finding.rs"]
mod reconcile_fix_finding;
#[path = "session_cmds_reconcile_git.rs"]
mod reconcile_git;
#[path = "session_cmds_reconcile_sidecars.rs"]
mod reconcile_sidecars;
use crate::session_cmds_reconcile_liveness::{
    ReconcileLivenessDecision, reconcile_liveness_decision,
};
#[cfg(test)]
use reconcile_sidecars::write_sidecar_atomically;
use reconcile_sidecars::{
    ArtifactRollbackGuard, persist_fix_finding_recovery_sidecar, persist_unpushed_commits_sidecar,
    rollback_sidecars,
};

type PersistSessionFn<'a> = dyn Fn(&Path, &MetaSessionState) -> Result<()> + 'a;

#[rustfmt::skip]
struct ArtifactRollbackLabels<'a> { removed_cleanup: &'a str, missing_after_match_cleanup: &'a str, remove_failed_cleanup: &'a str, preserved_cleanup: &'a str, missing_cleanup: &'a str, read_failed_cleanup: &'a str, artifact_label: &'a str }

struct SyntheticResultHooks<'a> {
    before_write: &'a dyn Fn(&Path),
    after_publish: &'a dyn Fn(&Path),
}

struct DaemonCompletionReconcileContext<'a> {
    project_root: &'a Path,
    session_id: &'a str,
    trigger: &'a str,
    session_dir: &'a Path,
    result_path: &'a Path,
    liveness: ReconcileLivenessDecision,
    hooks: SyntheticResultHooks<'a>,
    persist_session: &'a dyn Fn(&Path, &MetaSessionState) -> Result<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeadActiveSessionReconciliation {
    NoChange,
    DaemonCompletionFinalized,
    SynthesizedFailure,
    LateResultRetired,
}

#[rustfmt::skip]
impl DeadActiveSessionReconciliation {
    pub(crate) fn result_became_available(self) -> bool { matches!(self, Self::DaemonCompletionFinalized | Self::SynthesizedFailure | Self::LateResultRetired) }
    pub(crate) fn synthesized_failure(self) -> bool { matches!(self, Self::SynthesizedFailure) }
}

pub(crate) fn with_reconcile_lock<R>(
    session_dir: &Path,
    body: impl FnOnce() -> Result<R>,
) -> Result<Option<R>> {
    let lock_path = session_dir.join(".reconcile.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| {
            format!(
                "Failed to open reconciliation lock file: {}",
                lock_path.display()
            )
        })?;

    let mut lock = fd_lock::RwLock::new(file);
    match lock.try_write() {
        Ok(_guard) => Ok(Some(body()?)),
        Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(anyhow::Error::from(e).context("Failed to acquire reconciliation lock")),
    }
}

fn noop_path(_: &Path) {}

#[rustfmt::skip]
fn push_sidecar_rollback_guard(guards: &mut Vec<ArtifactRollbackGuard>, label: &str, result: Result<Option<ArtifactRollbackGuard>>, session_id: &str, trigger: &str) { match result { Ok(Some(rollback_guard)) => guards.push(rollback_guard), Ok(None) => {}, Err(err) => warn!(session_id = %session_id, trigger = %trigger, recovery_sidecar = %label, error = %err, "Failed to persist recovery sidecar"), } }

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
    .map(|opt| opt.unwrap_or(DeadActiveSessionReconciliation::NoChange))
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
    .map(|opt| opt.unwrap_or(DeadActiveSessionReconciliation::NoChange))
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
    let liveness = reconcile_liveness_decision(session_dir);
    let legacy_done = crate::session_cmds_daemon::legacy_complete_marker_is_valid(session_dir);
    if liveness.blocks_synthesis && !legacy_done {
        debug!(
            session_id = %session_id,
            trigger = %trigger,
            reconciliation_reason = %liveness.reason,
            "Dead-session reconciliation skipped because liveness signals still block synthetic fallback"
        );
        return Ok(false);
    }
    debug!(
        session_id = %session_id,
        trigger = %trigger,
        reconciliation_reason = %liveness.reason,
        "Dead-session reconciliation confirmed no pid/progress blocker before checking result.toml"
    );
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
    let liveness = reconcile_liveness_decision(session_dir);
    let legacy_done = crate::session_cmds_daemon::legacy_complete_marker_is_valid(session_dir);
    if liveness.blocks_synthesis && !legacy_done {
        debug!(
            session_id = %session_id,
            trigger = %trigger,
            reconciliation_reason = %liveness.reason,
            "Dead-session reconciliation re-check skipped synthetic fallback because liveness signals are still present"
        );
        return Ok(DeadActiveSessionReconciliation::NoChange);
    }
    debug!(
        session_id = %session_id,
        trigger = %trigger,
        reconciliation_reason = %liveness.reason,
        "Dead-session reconciliation re-check confirmed no pid/progress blocker before synthesizing fallback"
    );
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
    let daemon_completion_packet =
        match crate::session_cmds_daemon::load_daemon_completion_packet(session_dir) {
            Ok(packet) => packet,
            Err(err) => {
                warn!(
                    session_id = %session_id,
                    trigger = %trigger,
                    error = %err,
                    "Ignoring unusable daemon completion packet during dead-session reconciliation"
                );
                None
            }
        };
    let now = chrono::Utc::now();
    if let Some(packet) = daemon_completion_packet {
        return finalize_daemon_completion_during_reconcile(
            DaemonCompletionReconcileContext {
                project_root,
                session_id,
                trigger,
                session_dir,
                result_path: &result_path,
                liveness,
                hooks,
                persist_session,
            },
            session,
            packet,
            now,
            before_retire,
        );
    }
    let tool_name = session
        .tools
        .iter()
        .max_by_key(|(_, state)| state.updated_at)
        .map(|(tool, _)| tool.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let mut sidecar_rollback_guards = Vec::new();
    push_sidecar_rollback_guard(
        &mut sidecar_rollback_guards,
        "unpushed commit recovery",
        persist_unpushed_commits_sidecar(project_root, &session, session_dir),
        session_id,
        trigger,
    );
    push_sidecar_rollback_guard(
        &mut sidecar_rollback_guards,
        "fix-finding recovery",
        persist_fix_finding_recovery_sidecar(project_root, &session, session_dir),
        session_id,
        trigger,
    );
    #[rustfmt::skip]
    let artifacts = crate::pipeline_post_exec::collect_fallback_result_artifacts(project_root, session_id);
    let output_log_mtime = format_optional_file_mtime(&session_dir.join("output.log"));
    #[rustfmt::skip]
    let summary_prefix = reconcile_fix_finding::missing_result_summary_prefix(project_root, &session, session_dir, trigger, output_log_mtime.as_deref().unwrap_or("missing"), liveness.reason);
    let fallback = SessionResult {
        post_exec_gate: None,
        status: "failure".to_string(),
        exit_code: 1,
        summary: crate::pipeline_post_exec::build_fallback_result_summary(
            session_dir,
            &summary_prefix,
        ),
        tool: tool_name,
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: std::cmp::min(session.last_accessed, now),
        completed_at: now,
        events_count: 0,
        artifacts,
        ..Default::default()
    };
    #[rustfmt::skip]
    let result_contents = toml::to_string_pretty(&fallback).map_err(|err| anyhow!("Failed to serialize synthetic result for {session_id}: {err}"))?;
    match persist_new_result_file(&result_path, &result_contents, before_write)? {
        SyntheticResultPersistOutcome::AlreadyExists => {
            if let Err(err) = rollback_sidecars(&sidecar_rollback_guards) {
                warn!(
                    session_id = %session_id,
                    trigger = %trigger,
                    reconciliation_reason = "late_result_write_sidecar_cleanup_failed",
                    error = %err,
                    "Failed to clean up reconcile-owned unpushed commit sidecar after late result.toml write"
                );
            }
            let retired = retire_if_dead_with_result_impl(
                project_root,
                session_id,
                trigger,
                session_dir,
                liveness,
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
        rollback_reconciliation_artifacts(
            &result_path,
            result_contents.as_bytes(),
            &sidecar_rollback_guards,
        )
        .map_err(|cleanup_err| {
            anyhow!(
                "Failed to transition orphaned session to Retired phase during reconciliation for {session_id}: {err}; additionally failed to remove synthetic artifacts: {cleanup_err}"
            )
        })?;
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
        rollback_reconciliation_artifacts(
            &result_path,
            result_contents.as_bytes(),
            &sidecar_rollback_guards,
        )
        .map_err(|cleanup_err| {
            anyhow!(
                "Failed to persist retired orphaned session state for {session_id}: {err}; additionally failed to remove synthetic artifacts: {cleanup_err}"
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

fn finalize_daemon_completion_during_reconcile<B>(
    context: DaemonCompletionReconcileContext<'_>,
    mut session: MetaSessionState,
    packet: crate::session_cmds_daemon::DaemonCompletionPacket,
    completed_at: chrono::DateTime<chrono::Utc>,
    before_retire: B,
) -> Result<DeadActiveSessionReconciliation>
where
    B: FnOnce(&mut MetaSessionState),
{
    let DaemonCompletionReconcileContext {
        project_root,
        session_id,
        trigger,
        session_dir,
        result_path,
        liveness,
        hooks,
        persist_session,
    } = context;
    let SyntheticResultHooks {
        before_write,
        after_publish,
    } = hooks;
    if let Some(reason) = packet.reason.as_deref() {
        session
            .termination_reason
            .get_or_insert_with(|| reason.to_string());
    }
    let result = crate::session_cmds_daemon::daemon_completion_result(
        project_root,
        session_dir,
        &session,
        &packet,
        completed_at,
    );
    #[rustfmt::skip]
    let result_contents = crate::session_kill_diagnostics::signal_toml(&result, &session, session_id, session_dir, packet.exit_code).context("serialize result")?;
    match persist_new_result_file(result_path, &result_contents, before_write)? {
        SyntheticResultPersistOutcome::AlreadyExists => {
            let retired = retire_if_dead_with_result_impl(
                project_root,
                session_id,
                trigger,
                session_dir,
                liveness,
                persist_session,
            )?;
            info!(
                session_id = %session_id,
                trigger = %trigger,
                reconciliation_reason = "late_result_write",
                result_path = %result_path.display(),
                result_mtime = %format_optional_file_mtime(result_path).unwrap_or_else(|| "unknown".to_string()),
                "Late result.toml write won"
            );
            return Ok(if retired {
                DeadActiveSessionReconciliation::LateResultRetired
            } else {
                DeadActiveSessionReconciliation::NoChange
            });
        }
        SyntheticResultPersistOutcome::Created => {}
    }

    after_publish(result_path);
    before_retire(&mut session);
    if !crate::session_cmds_daemon::retire_session_from_daemon_completion(
        &mut session,
        &packet,
        completed_at,
    ) {
        rollback_reconciliation_artifacts(result_path, result_contents.as_bytes(), &[])
            .map_err(|cleanup_err| {
                anyhow!(
                    "Failed to transition daemon-completed session to Retired phase during reconciliation for {session_id}; additionally failed to remove daemon completion result: {cleanup_err}"
                )
            })?;
        return Err(anyhow!(
            "Failed to transition daemon-completed session to Retired phase during reconciliation for {session_id}"
        ));
    }
    packet.persist_review_diag(
        result_path,
        result_contents.as_bytes(),
        session_dir,
        &session,
    );
    if let Err(err) = persist_session(session_dir, &session) {
        warn!(
            session_id = %session_id,
            trigger = %trigger,
            reconciliation_reason = "daemon_completion",
            error = %err,
            "Failed to persist retired daemon-completed session state during reconciliation"
        );
        return Ok(DeadActiveSessionReconciliation::DaemonCompletionFinalized);
    }
    csa_session::write_cooldown_marker_from_session_dir(session_dir, session_id, completed_at);
    warn!(
        session_id = %session_id,
        trigger = %trigger,
        reconciliation_reason = "daemon_completion",
        result_path = %result_path.display(),
        exit_code = packet.exit_code,
        status = %packet.status,
        "Recovered daemon-completed session"
    );
    Ok(DeadActiveSessionReconciliation::DaemonCompletionFinalized)
}

pub(crate) fn retire_if_dead_with_result(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
) -> Result<bool> {
    let session_dir = get_session_dir(project_root, session_id)?;
    let Some(liveness) =
        dead_session_with_result_needs_retire(project_root, session_id, &session_dir)?
    else {
        return Ok(false);
    };
    with_reconcile_lock(&session_dir, || {
        retire_if_dead_with_result_impl(
            project_root,
            session_id,
            trigger,
            &session_dir,
            liveness,
            &persist_session_state_atomically,
        )
    })
    .map(|opt| opt.unwrap_or(false))
}

fn dead_session_with_result_needs_retire(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
) -> Result<Option<ReconcileLivenessDecision>> {
    let session = load_session(project_root, session_id)?;
    if !matches!(session.phase, SessionPhase::Active) {
        return Ok(None);
    }
    let liveness = reconcile_liveness_decision(session_dir);
    let legacy_done = crate::session_cmds_daemon::legacy_complete_marker_is_valid(session_dir);
    if liveness.blocks_synthesis && !legacy_done {
        debug!(
            session_id = %session_id,
            reason = %liveness.reason,
            "Dead-session retirement deferred: progress/liveness still detected"
        );
        return Ok(None);
    }
    if load_result(project_root, session_id)?.is_none() {
        return Ok(None);
    }
    Ok(Some(liveness))
}

fn retire_if_dead_with_result_impl(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
    session_dir: &Path,
    liveness: ReconcileLivenessDecision,
    persist_session: &PersistSessionFn<'_>,
) -> Result<bool> {
    let mut session = load_session(project_root, session_id)?;
    if !matches!(session.phase, SessionPhase::Active) {
        return Ok(false);
    }
    let legacy_done = crate::session_cmds_daemon::legacy_complete_marker_is_valid(session_dir);
    if liveness.blocks_synthesis && !legacy_done {
        debug!(
            session_id = %session_id,
            reason = %liveness.reason,
            "retire_if_dead_with_result: progress/liveness detected, skipping retirement"
        );
        return Ok(false);
    }
    if load_result(project_root, session_id)?.is_none() {
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
    reconcile_cleanup::cleanup_retired_session_target_dir(session_dir)?;
    info!(
        session_id = %session_id,
        trigger = %trigger,
        "Retired dead Active session with result"
    );
    Ok(true)
}

pub(crate) fn persist_session_state_atomically(
    session_dir: &Path,
    session: &MetaSessionState,
) -> Result<()> {
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
#[rustfmt::skip]
fn remove_synthetic_result_if_unchanged(result_path: &Path, expected_contents: &[u8]) -> std::io::Result<()> {
    remove_artifact_if_unchanged(result_path, expected_contents, ArtifactRollbackLabels { removed_cleanup: "removed_synthetic_result", missing_after_match_cleanup: "result_missing_after_match", remove_failed_cleanup: "remove_failed", preserved_cleanup: "late_real_result_preserved", missing_cleanup: "result_missing", read_failed_cleanup: "read_failed", artifact_label: "synthetic result.toml" })
}
fn rollback_reconciliation_artifacts(
    result_path: &Path,
    result_contents: &[u8],
    sidecar_rollback_guards: &[ArtifactRollbackGuard],
) -> std::io::Result<()> {
    remove_synthetic_result_if_unchanged(result_path, result_contents)?;
    rollback_sidecars(sidecar_rollback_guards)
}
#[rustfmt::skip]
fn remove_artifact_if_unchanged(artifact_path: &Path, expected_contents: &[u8], labels: ArtifactRollbackLabels<'_>) -> std::io::Result<()> {
    match fs::read(artifact_path) {
        Ok(current_contents) if current_contents == expected_contents => match fs::remove_file(artifact_path) {
            Ok(()) => { warn!(artifact_path = %artifact_path.display(), rollback_cleanup = labels.removed_cleanup, "Rollback removed matching {} after reconciliation failure", labels.artifact_label); Ok(()) }
            Err(err) if err.kind() == ErrorKind::NotFound => { warn!(artifact_path = %artifact_path.display(), rollback_cleanup = labels.missing_after_match_cleanup, "Rollback matching {} was already absent after reconciliation failure", labels.artifact_label); Ok(()) }
            Err(err) => { warn!(artifact_path = %artifact_path.display(), rollback_cleanup = labels.remove_failed_cleanup, error = %err, "Rollback failed to remove matching {} after reconciliation failure", labels.artifact_label); Err(err) }
        },
        Ok(_) if labels.preserved_cleanup == "late_real_result_preserved" => { warn!(artifact_path = %artifact_path.display(), rollback_cleanup = labels.preserved_cleanup, "Rollback detected late real result.toml and left it in place"); Ok(()) }
        Ok(_) => { warn!(artifact_path = %artifact_path.display(), rollback_cleanup = labels.preserved_cleanup, "Rollback preserved {} because contents changed after reconciliation failure", labels.artifact_label); Ok(()) }
        Err(err) if err.kind() == ErrorKind::NotFound => { warn!(artifact_path = %artifact_path.display(), rollback_cleanup = labels.missing_cleanup, "Rollback found no {} to clean up after reconciliation failure", labels.artifact_label); Ok(()) }
        Err(err) => { warn!(artifact_path = %artifact_path.display(), rollback_cleanup = labels.read_failed_cleanup, error = %err, "Rollback failed to read {} for content-aware cleanup after reconciliation failure", labels.artifact_label); Ok(()) }
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
    before_write(result_path);
    publish_result_file_if_absent(result_path, contents, "synthetic result")
}

#[cfg(test)]
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
    publish_result_file_if_absent_with_writer(
        result_path,
        contents,
        "synthetic result",
        write_contents,
    )
}

fn format_optional_file_mtime(path: &Path) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let modified = chrono::DateTime::<chrono::Utc>::from(modified);
    Some(modified.to_rfc3339())
}

#[cfg(test)]
#[path = "session_cmds_reconcile_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "session_cmds_reconcile_diagnostics_tests.rs"]
mod diagnostics_tests;

#[cfg(test)]
#[path = "session_cmds_reconcile_tests_tail.rs"]
mod tail_tests;

#[cfg(test)]
#[path = "session_cmds_reconcile_progress_tests.rs"]
mod progress_tests;
