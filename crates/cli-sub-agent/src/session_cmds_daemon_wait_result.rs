use std::fs;
use std::path::Path;

use anyhow::Result;

use super::super::session_has_terminal_process;

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

    Ok(Some(result))
}

/// Refresh result via session_dir for cross-project sessions or via project_root otherwise.
pub(super) fn refresh_result_for_wait(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
    is_cross_project: bool,
) -> Result<Option<csa_session::SessionResult>> {
    if is_cross_project {
        crate::session_observability::refresh_and_repair_result_from_dir(session_dir)
    } else {
        crate::session_observability::refresh_and_repair_result(project_root, session_id)
    }
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
        Ok(Some(result))
    } else {
        load_completed_daemon_result(project_root, session_id, session_dir)
    }
}

fn load_output_result_fallback(session_dir: &Path) -> Result<Option<csa_session::SessionResult>> {
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
    Ok(Some(result))
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
        && let Some(output_result) = load_output_result_fallback(session_dir)?
    {
        tracing::info!(
            session_id,
            "Session completion detected via output/result.toml fallback"
        );
        return Ok(Some(output_result));
    }

    Ok(None)
}
