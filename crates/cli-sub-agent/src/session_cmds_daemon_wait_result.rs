use std::fs;
use std::path::Path;

use anyhow::Result;

use super::super::session_has_terminal_process;

pub(super) fn suppress_pending_tier_failover_result(
    session_id: &str,
    session_dir: &Path,
    result: csa_session::SessionResult,
) -> Option<csa_session::SessionResult> {
    if crate::session_tier_failover::is_pending_tier_failover_handoff(session_dir, &result)
        || is_probable_pending_tier_failover_failure(session_dir, &result)
    {
        tracing::debug!(
            session_id,
            status = %result.status,
            "Ignoring intermediate tier-failover result while fallback handoff is still live"
        );
        None
    } else {
        Some(result)
    }
}

fn is_probable_pending_tier_failover_failure(
    session_dir: &Path,
    result: &csa_session::SessionResult,
) -> bool {
    result.status == "failure"
        && result.exit_code != 0
        && result.tool == "gemini-cli"
        && result.summary.to_ascii_lowercase().contains("status: 400")
        && session_has_review_tier_context(session_dir)
        && tier_failover_handoff_has_liveness(session_dir)
}

fn session_has_review_tier_context(session_dir: &Path) -> bool {
    let state_path = session_dir.join("state.toml");
    let Ok(contents) = fs::read_to_string(state_path) else {
        return false;
    };
    let Ok(session) = toml::from_str::<csa_session::MetaSessionState>(&contents) else {
        return false;
    };
    session.task_context.task_type.as_deref() == Some("reviewer_sub_session")
        && session.task_context.tier_name.is_some()
}

fn tier_failover_handoff_has_liveness(session_dir: &Path) -> bool {
    csa_process::ToolLiveness::has_live_process(session_dir)
        || csa_process::ToolLiveness::daemon_pid_is_alive(session_dir)
        || csa_process::ToolLiveness::is_alive(session_dir)
}

fn load_completed_daemon_result(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
) -> Result<Option<csa_session::SessionResult>> {
    let daemon_alive_at_refresh_start = session_has_terminal_process(session_dir);
    let result =
        match crate::session_observability::refresh_and_repair_result(project_root, session_id) {
            Ok(Some(result)) => result,
            Ok(None) => return Ok(None),
            Err(err) if daemon_alive_at_refresh_start => {
                tracing::debug!(
                    session_id,
                    error = %err,
                    "Ignoring transient result refresh failure while daemon is still alive"
                );
                return Ok(None);
            }
            Err(err) => return Err(err),
        };

    Ok(suppress_pending_tier_failover_result(
        session_id,
        session_dir,
        result,
    ))
}

/// Refresh result via session_dir for cross-project sessions or via project_root otherwise.
pub(super) fn refresh_result_for_wait(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
    is_cross_project: bool,
) -> Result<Option<csa_session::SessionResult>> {
    let result = if is_cross_project {
        crate::session_observability::refresh_and_repair_result_from_dir(session_dir)
    } else {
        crate::session_observability::refresh_and_repair_result(project_root, session_id)
    }?;
    Ok(result
        .and_then(|result| suppress_pending_tier_failover_result(session_id, session_dir, result)))
}

fn load_completed_daemon_result_adaptive(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
    is_cross_project: bool,
) -> Result<Option<csa_session::SessionResult>> {
    if is_cross_project {
        let daemon_alive_at_refresh_start = session_has_terminal_process(session_dir);
        let result = match crate::session_observability::refresh_and_repair_result_from_dir(
            session_dir,
        ) {
            Ok(Some(result)) => result,
            Ok(None) => return Ok(None),
            Err(err) if daemon_alive_at_refresh_start => {
                tracing::debug!(
                    session_id,
                    error = %err,
                    "Ignoring transient result refresh failure (cross-project) while daemon is still alive"
                );
                return Ok(None);
            }
            Err(err) => return Err(err),
        };
        Ok(suppress_pending_tier_failover_result(
            session_id,
            session_dir,
            result,
        ))
    } else {
        load_completed_daemon_result(project_root, session_id, session_dir)
    }
}

fn load_output_result_fallback(
    session_id: &str,
    session_dir: &Path,
) -> Result<Option<csa_session::SessionResult>> {
    let output_result_path = session_dir
        .join("output")
        .join(csa_session::result::RESULT_FILE_NAME);
    if !output_result_path.is_file() {
        return Ok(None);
    }

    tracing::debug!(
        path = %output_result_path.display(),
        "Found output/result.toml as fallback completion signal"
    );

    let contents = fs::read_to_string(&output_result_path)?;
    let result: csa_session::SessionResult = toml::from_str(&contents)?;
    Ok(suppress_pending_tier_failover_result(
        session_id,
        session_dir,
        result,
    ))
}

pub(super) fn load_completed_daemon_result_with_fallback(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
    is_cross_project: bool,
) -> Result<Option<csa_session::SessionResult>> {
    if let Some(result) = load_completed_daemon_result_adaptive(
        project_root,
        session_id,
        session_dir,
        is_cross_project,
    )? {
        return Ok(Some(result));
    }

    if !session_has_terminal_process(session_dir)
        && let Some(output_result) = load_output_result_fallback(session_id, session_dir)?
    {
        tracing::info!(
            session_id,
            "Session completion detected via output/result.toml fallback"
        );
        return Ok(Some(output_result));
    }

    Ok(None)
}
