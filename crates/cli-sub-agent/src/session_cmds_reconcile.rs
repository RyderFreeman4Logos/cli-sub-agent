use anyhow::{Context, Result, anyhow};
use csa_core::vcs::VcsKind;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

use csa_process::ToolLiveness;
use csa_session::{
    MetaSessionState, SessionPhase, SessionResult, get_session_dir, load_result, load_session,
};

use crate::plan_cmd::shell_escape_for_command;
#[path = "session_cmds_reconcile_cleanup.rs"]
mod reconcile_cleanup;
#[path = "session_cmds_reconcile_liveness.rs"]
mod reconcile_liveness;
use reconcile_liveness::reconcile_liveness_decision;

type PersistSessionFn<'a> = dyn Fn(&Path, &MetaSessionState) -> Result<()> + 'a;
const UNPUSHED_COMMITS_SIDECAR_PATH: &str = "output/unpushed_commits.json";

#[rustfmt::skip]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct UnpushedCommitRecord { sha: String, subject: String }

#[rustfmt::skip]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct UnpushedCommitsSidecar { branch: String, remote_ref: Option<String>, commits_ahead: u64, commits: Vec<UnpushedCommitRecord>, recovery_command: String }

#[rustfmt::skip]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ArtifactRollbackGuard { artifact_path: PathBuf, expected_contents: Vec<u8>, rollback_action: ArtifactRollbackAction }

#[rustfmt::skip]
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArtifactRollbackAction { RemoveIfContentsMatch, RestoreOriginal(Vec<u8>) }

#[rustfmt::skip]
struct ArtifactRollbackLabels<'a> { removed_cleanup: &'a str, missing_after_match_cleanup: &'a str, remove_failed_cleanup: &'a str, preserved_cleanup: &'a str, missing_cleanup: &'a str, read_failed_cleanup: &'a str, artifact_label: &'a str }

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

fn with_reconcile_lock<R>(
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

fn git_output(project_root: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .with_context(|| format!("Failed to run git {:?}", args))
}

fn git_success(project_root: &Path, args: &[&str]) -> bool {
    git_output(project_root, args)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn inspect_unpushed_commits(
    project_root: &Path,
    branch: &str,
) -> Result<Option<UnpushedCommitsSidecar>> {
    let session_branch_ref = format!("refs/heads/{branch}");
    let range = if git_success(
        project_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/origin/{branch}"),
        ],
    ) {
        (
            Some(format!("origin/{branch}")),
            format!("origin/{branch}..{session_branch_ref}"),
        )
    } else if git_success(
        project_root,
        &["rev-parse", "--verify", "--quiet", "refs/heads/main"],
    ) {
        (None, format!("main..{session_branch_ref}"))
    } else if git_success(
        project_root,
        &["rev-parse", "--verify", "--quiet", "refs/heads/master"],
    ) {
        (None, format!("master..{session_branch_ref}"))
    } else {
        return Ok(None);
    };
    let (remote_ref, rev_range) = range;

    let count_output = git_output(project_root, &["rev-list", "--count", &rev_range])?;
    if !count_output.status.success() {
        return Ok(None);
    }
    let commits_ahead = String::from_utf8_lossy(&count_output.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0);
    if commits_ahead == 0 {
        return Ok(None);
    }

    let log_output = git_output(project_root, &["log", "--format=%H%x09%s", &rev_range])?;
    if !log_output.status.success() {
        return Ok(None);
    }

    let commits = String::from_utf8_lossy(&log_output.stdout)
        .lines()
        .filter_map(|line| {
            let (sha, subject) = line.split_once('\t')?;
            Some(UnpushedCommitRecord {
                sha: sha.to_string(),
                subject: subject.to_string(),
            })
        })
        .collect::<Vec<_>>();
    if commits.is_empty() {
        return Ok(None);
    }

    Ok(Some(UnpushedCommitsSidecar {
        branch: branch.to_string(),
        remote_ref,
        commits_ahead,
        commits,
        recovery_command: format_git_push_recovery_command(branch),
    }))
}

#[rustfmt::skip]
fn persist_unpushed_commits_sidecar(project_root: &Path, session: &MetaSessionState, session_dir: &Path) -> Result<Option<ArtifactRollbackGuard>> {
    if session.resolved_identity().vcs_kind != VcsKind::Git { return Ok(None); }
    let Some(branch) = session.branch.as_deref() else { return Ok(None); };
    let Some(sidecar) = inspect_unpushed_commits(project_root, branch)? else { return Ok(None); };
    fs::create_dir_all(session_dir.join("output"))?;
    let sidecar_path = session_dir.join(UNPUSHED_COMMITS_SIDECAR_PATH);
    let sidecar_contents = serde_json::to_vec_pretty(&sidecar)?;
    let rollback_guard = artifact_rollback_guard(&sidecar_path, sidecar_contents.as_slice())?;
    write_sidecar_atomically(&sidecar_path, &sidecar_contents)?;
    Ok(rollback_guard)
}

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
    if liveness.blocks_synthesis {
        info!(
            session_id = %session_id,
            trigger = %trigger,
            reconciliation_reason = %liveness.reason,
            "Dead-session reconciliation skipped because liveness signals still block synthetic fallback"
        );
        return Ok(false);
    }
    info!(
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
    if liveness.blocks_synthesis {
        info!(
            session_id = %session_id,
            trigger = %trigger,
            reconciliation_reason = %liveness.reason,
            "Dead-session reconciliation re-check skipped synthetic fallback because liveness signals are still present"
        );
        return Ok(DeadActiveSessionReconciliation::NoChange);
    }
    info!(
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
    let now = chrono::Utc::now();
    let tool_name = session
        .tools
        .iter()
        .max_by_key(|(_, state)| state.updated_at)
        .map(|(tool, _)| tool.clone())
        .unwrap_or_else(|| "unknown".to_string());
    #[rustfmt::skip]
    let sidecar_rollback_guard = match persist_unpushed_commits_sidecar(project_root, &session, session_dir) {
        Ok(rollback_guard) => rollback_guard,
        Err(err) => {
            warn!(
                session_id = %session_id,
                trigger = %trigger,
                error = %err,
                "Failed to persist unpushed commit recovery sidecar"
            );
            None
        }
    };
    #[rustfmt::skip]
    let artifacts = crate::pipeline_post_exec::collect_fallback_result_artifacts(project_root, session_id);
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
    #[rustfmt::skip]
    let result_contents = toml::to_string_pretty(&fallback).map_err(|err| anyhow!("Failed to serialize synthetic result for {session_id}: {err}"))?;
    match persist_new_result_file(&result_path, &result_contents, before_write)? {
        SyntheticResultPersistOutcome::AlreadyExists => {
            if let Err(err) = rollback_sidecar(sidecar_rollback_guard.as_ref()) {
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
            sidecar_rollback_guard.as_ref(),
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
            sidecar_rollback_guard.as_ref(),
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
    .map(|opt| opt.unwrap_or(false))
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
    if ToolLiveness::has_live_process(session_dir) || ToolLiveness::daemon_pid_is_alive(session_dir)
    {
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
        || ToolLiveness::daemon_pid_is_alive(session_dir)
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
    reconcile_cleanup::cleanup_retired_session_target_dir(session_dir)?;
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
#[rustfmt::skip]
fn remove_synthetic_result_if_unchanged(result_path: &Path, expected_contents: &[u8]) -> std::io::Result<()> {
    remove_artifact_if_unchanged(result_path, expected_contents, ArtifactRollbackLabels { removed_cleanup: "removed_synthetic_result", missing_after_match_cleanup: "result_missing_after_match", remove_failed_cleanup: "remove_failed", preserved_cleanup: "late_real_result_preserved", missing_cleanup: "result_missing", read_failed_cleanup: "read_failed", artifact_label: "synthetic result.toml" })
}
#[rustfmt::skip]
fn rollback_sidecar(rollback_guard: Option<&ArtifactRollbackGuard>) -> std::io::Result<()> {
    let Some(rollback_guard) = rollback_guard else { return Ok(()); };
    match &rollback_guard.rollback_action {
        ArtifactRollbackAction::RemoveIfContentsMatch => remove_artifact_if_unchanged(&rollback_guard.artifact_path, rollback_guard.expected_contents.as_slice(), ArtifactRollbackLabels { removed_cleanup: "removed_unpushed_commits_sidecar", missing_after_match_cleanup: "sidecar_missing_after_match", remove_failed_cleanup: "sidecar_remove_failed", preserved_cleanup: "preexisting_sidecar_preserved", missing_cleanup: "sidecar_missing", read_failed_cleanup: "sidecar_read_failed", artifact_label: "unpushed_commits.json sidecar" }),
        ArtifactRollbackAction::RestoreOriginal(original_contents) => match fs::read(&rollback_guard.artifact_path) {
            Ok(current_contents) if current_contents == rollback_guard.expected_contents => { fs::write(&rollback_guard.artifact_path, original_contents)?; warn!(artifact_path = %rollback_guard.artifact_path.display(), rollback_cleanup = "restored_preexisting_unpushed_commits_sidecar", "Rollback restored preexisting unpushed_commits.json sidecar after reconciliation failure"); Ok(()) }
            Ok(_) => { warn!(artifact_path = %rollback_guard.artifact_path.display(), rollback_cleanup = "preexisting_sidecar_preserved", "Rollback preserved unpushed_commits.json sidecar because contents changed after reconciliation failure"); Ok(()) }
            Err(err) if err.kind() == ErrorKind::NotFound => { warn!(artifact_path = %rollback_guard.artifact_path.display(), rollback_cleanup = "sidecar_missing", "Rollback found no unpushed_commits.json sidecar to restore after reconciliation failure"); Ok(()) }
            Err(err) => { warn!(artifact_path = %rollback_guard.artifact_path.display(), rollback_cleanup = "sidecar_read_failed", error = %err, "Rollback failed to read unpushed_commits.json sidecar for content-aware restore after reconciliation failure"); Ok(()) }
        },
    }
}
#[rustfmt::skip]
fn rollback_reconciliation_artifacts(result_path: &Path, result_contents: &[u8], sidecar_rollback_guard: Option<&ArtifactRollbackGuard>) -> std::io::Result<()> {
    remove_synthetic_result_if_unchanged(result_path, result_contents)?;
    rollback_sidecar(sidecar_rollback_guard)
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

#[rustfmt::skip]
fn artifact_rollback_guard(artifact_path: &Path, expected_contents: &[u8]) -> std::io::Result<Option<ArtifactRollbackGuard>> {
    match fs::read(artifact_path) {
        Ok(current_contents) if current_contents == expected_contents => Ok(None),
        Ok(current_contents) => Ok(Some(ArtifactRollbackGuard {
            artifact_path: artifact_path.to_path_buf(),
            expected_contents: expected_contents.to_vec(),
            rollback_action: ArtifactRollbackAction::RestoreOriginal(current_contents),
        })),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(Some(ArtifactRollbackGuard {
            artifact_path: artifact_path.to_path_buf(),
            expected_contents: expected_contents.to_vec(),
            rollback_action: ArtifactRollbackAction::RemoveIfContentsMatch,
        })),
        Err(err) => Err(err),
    }
}

#[rustfmt::skip]
fn write_sidecar_atomically(sidecar_path: &Path, contents: &[u8]) -> Result<()> {
    let sidecar_dir = sidecar_path.parent().ok_or_else(|| anyhow!("Unpushed commit sidecar path has no parent: {}", sidecar_path.display()))?;
    let mut temp_file = tempfile::NamedTempFile::new_in(sidecar_dir).with_context(|| format!("Failed to create temporary unpushed commit sidecar in {}", sidecar_dir.display()))?;
    temp_file.as_file_mut().write_all(contents).with_context(|| format!("Failed to write temporary unpushed commit sidecar for {}", sidecar_path.display()))?;
    temp_file.as_file_mut().sync_all().with_context(|| format!("Failed to sync temporary unpushed commit sidecar for {}", sidecar_path.display()))?;
    preserve_existing_permissions_if_present(temp_file.as_file_mut(), sidecar_path, "unpushed commit sidecar")?;
    temp_file.persist(sidecar_path).map_err(|err| anyhow!("Failed to publish unpushed commit sidecar {}: {}", sidecar_path.display(), err.error))?;
    Ok(())
}

#[rustfmt::skip]
fn format_git_push_recovery_command(branch: &str) -> String {
    if branch_is_shell_word_safe(branch) {
        format!("git push -u origin {branch}")
    } else {
        format!("git push -u origin {}", shell_escape_for_command(branch))
    }
}

#[rustfmt::skip]
fn branch_is_shell_word_safe(branch: &str) -> bool { branch.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')) }

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

#[cfg(test)]
#[path = "session_cmds_reconcile_tests_tail.rs"]
mod tail_tests;

#[cfg(test)]
#[path = "session_cmds_reconcile_progress_tests.rs"]
mod progress_tests;
