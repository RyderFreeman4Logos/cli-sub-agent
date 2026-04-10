use anyhow::{Result, anyhow};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;
use tracing::{info, warn};

use csa_session::{
    SessionPhase, SessionResult, get_session_dir, load_result, load_session, save_session_in,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeadActiveSessionReconciliation {
    NoChange,
    SynthesizedFailure,
    LateResultRetired,
}

impl DeadActiveSessionReconciliation {
    pub(crate) fn result_became_available(self) -> bool {
        matches!(self, Self::SynthesizedFailure | Self::LateResultRetired)
    }

    pub(crate) fn synthesized_failure(self) -> bool {
        matches!(self, Self::SynthesizedFailure)
    }
}

pub(crate) fn ensure_terminal_result_for_dead_active_session(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
) -> Result<DeadActiveSessionReconciliation> {
    ensure_terminal_result_for_dead_active_session_impl(project_root, session_id, trigger, |_| {})
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
    ensure_terminal_result_for_dead_active_session_impl(
        project_root,
        session_id,
        trigger,
        before_write,
    )
}

fn ensure_terminal_result_for_dead_active_session_impl<F>(
    project_root: &Path,
    session_id: &str,
    trigger: &str,
    before_write: F,
) -> Result<DeadActiveSessionReconciliation>
where
    F: FnOnce(&Path),
{
    let mut session = load_session(project_root, session_id)?;
    if !matches!(session.phase, SessionPhase::Active) {
        return Ok(DeadActiveSessionReconciliation::NoChange);
    }
    let session_dir = get_session_dir(project_root, session_id)?;
    if csa_process::ToolLiveness::has_live_process(&session_dir) {
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
            &session_dir,
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
            let retired = retire_if_dead_with_result(project_root, session_id, trigger)?;
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

    csa_session::write_cooldown_marker_from_session_dir(
        &session_dir,
        session_id,
        fallback.completed_at,
    );
    session.termination_reason = Some("orphaned_process".to_string());
    let _ = session.apply_phase_event(csa_session::PhaseEvent::Retired);
    let session_root = session_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow!("Invalid session dir layout: {}", session_dir.display()))?;
    save_session_in(session_root, &session)?;
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
    let mut session = load_session(project_root, session_id)?;
    if !matches!(session.phase, SessionPhase::Active) {
        return Ok(false);
    }
    let session_dir = get_session_dir(project_root, session_id)?;
    if csa_process::ToolLiveness::has_live_process(&session_dir)
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
    let session_root = session_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow!("Invalid session dir layout: {}", session_dir.display()))?;
    save_session_in(session_root, &session)?;
    info!(
        session_id = %session_id,
        trigger = %trigger,
        "Retired dead Active session with result"
    );
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyntheticResultPersistOutcome {
    Created,
    AlreadyExists,
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
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(result_path)
    {
        Ok(mut file) => {
            file.write_all(contents.as_bytes()).map_err(|err| {
                anyhow!(
                    "Failed to write synthetic result for {}: {err}",
                    result_path.display()
                )
            })?;
            file.sync_all().map_err(|err| {
                anyhow!(
                    "Failed to sync synthetic result for {}: {err}",
                    result_path.display()
                )
            })?;
            Ok(SyntheticResultPersistOutcome::Created)
        }
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            Ok(SyntheticResultPersistOutcome::AlreadyExists)
        }
        Err(err) => Err(anyhow!(
            "Failed to create synthetic result for {}: {err}",
            result_path.display()
        )),
    }
}

fn format_optional_file_mtime(path: &Path) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let modified = chrono::DateTime::<chrono::Utc>::from(modified);
    Some(modified.to_rfc3339())
}
