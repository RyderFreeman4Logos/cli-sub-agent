use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{
    AttachPrimaryOutput, attach_primary_output_for_session, load_daemon_completion_packet,
    read_daemon_pid, session_has_terminal_process,
};

const ATTACH_ROUTE_RESOLUTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const ATTACH_ROUTE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

pub(super) fn wait_for_attach_live_output_path<E, S>(
    session_dir: &Path,
    session_id: &str,
    stdout_path: &Path,
    output_path: &Path,
    mut elapsed: E,
    mut sleep: S,
) -> Result<Option<PathBuf>>
where
    E: FnMut() -> std::time::Duration,
    S: FnMut(std::time::Duration),
{
    loop {
        let primary_output = attach_primary_output_for_session(session_dir);
        let candidate = match primary_output {
            AttachPrimaryOutput::StdoutLog => stdout_path,
            AttachPrimaryOutput::OutputLog => output_path,
            AttachPrimaryOutput::Pending => {
                if elapsed() > ATTACH_ROUTE_RESOLUTION_TIMEOUT {
                    anyhow::bail!(
                        "session output routing not resolved after 30s — session {session_id} may still be starting",
                    );
                }
                sleep(ATTACH_ROUTE_POLL_INTERVAL);
                continue;
            }
        };

        if candidate.exists() {
            return Ok(Some(candidate.to_path_buf()));
        }

        if primary_output == AttachPrimaryOutput::OutputLog {
            if !session_has_terminal_process(session_dir) {
                return Ok(None);
            }
            sleep(ATTACH_ROUTE_POLL_INTERVAL);
            continue;
        }

        if elapsed() > ATTACH_ROUTE_RESOLUTION_TIMEOUT {
            let missing_name = candidate
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("session output");
            anyhow::bail!(
                "{missing_name} not found after 30s — session {session_id} may not be a daemon session",
            );
        }

        sleep(ATTACH_ROUTE_POLL_INTERVAL);
    }
}

pub(super) fn resolve_attach_terminal_exit(
    project_root: &Path,
    session_dir: &Path,
    session_id: &str,
) -> Result<i32> {
    if let Some(completion) = load_daemon_completion_packet(session_dir)?
        && !session_has_terminal_process(session_dir)
    {
        return Ok(completion.exit_code);
    }

    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    if result_path.exists() && !session_has_terminal_process(session_dir) {
        let exit_code = fs::read_to_string(&result_path)
            .ok()
            .and_then(|s| toml::from_str::<csa_session::result::SessionResult>(&s).ok())
            .map(|r| r.exit_code)
            .unwrap_or(0);
        return Ok(exit_code);
    }

    if !session_has_terminal_process(session_dir) {
        if let Some(pid) = read_daemon_pid(session_dir) {
            eprintln!(
                "Daemon process {} exited without producing result.toml; synthesizing fallback",
                pid,
            );
        } else {
            eprintln!(
                "Session {} has no live daemon process and no result.toml; synthesizing fallback",
                session_id,
            );
        }
        // For cross-project sessions, skip synthesis (requires project_root write access).
        let is_cross_project = csa_session::get_session_dir(project_root, session_id).is_err();
        if !is_cross_project {
            let _ = crate::session_cmds::ensure_terminal_result_for_dead_active_session(
                project_root,
                session_id,
                "session attach (daemon dead)",
            );
        }
        if result_path.is_file()
            && let Ok(contents) = fs::read_to_string(&result_path)
            && let Ok(result) = toml::from_str::<csa_session::result::SessionResult>(&contents)
        {
            return Ok(result.exit_code);
        }
        return Ok(1);
    }

    anyhow::bail!(
        "session {session_id} has not reached a terminal state while resolving attach output",
    )
}
