use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use csa_executor::{TransportFactory, TransportMode};

use super::{
    AttachPrimaryOutput, load_daemon_completion_packet, read_daemon_pid,
    session_has_terminal_process,
};

pub(super) const ATTACH_METADATA_STDOUT_GRACE_WINDOW: std::time::Duration =
    std::time::Duration::from_secs(3);
const ATTACH_ROUTE_RESOLUTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const ATTACH_ROUTE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);
const ATTACH_METADATA_RETRY_MAX_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
const ATTACH_STDOUT_LIVE_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);

fn runtime_binary_file_name(runtime_binary: &str) -> Option<&str> {
    std::path::Path::new(runtime_binary)
        .file_name()
        .and_then(|name| name.to_str())
}

pub(super) fn runtime_binary_indicates_codex_acp_file_name(file_name: &str) -> bool {
    file_name.contains("codex-acp")
}

pub(super) fn runtime_binary_indicates_codex_acp(runtime_binary: &str) -> bool {
    runtime_binary_file_name(runtime_binary)
        .is_some_and(runtime_binary_indicates_codex_acp_file_name)
}

fn routes_session_output_to_output_log(metadata: &csa_session::metadata::SessionMetadata) -> bool {
    if metadata.tool == "codex" {
        return metadata
            .runtime_binary
            .as_deref()
            .is_some_and(runtime_binary_indicates_codex_acp);
    }
    matches!(
        TransportFactory::mode_for_tool(&metadata.tool),
        TransportMode::Acp
    )
}

#[cfg_attr(test, allow(dead_code))]
pub(super) fn attach_primary_output_from_metadata(
    metadata: &csa_session::metadata::SessionMetadata,
    output_log_exists: bool,
    session_active: bool,
) -> AttachPrimaryOutput {
    if metadata.tool == "codex" && metadata.runtime_binary.is_none() {
        return if !session_active {
            if output_log_exists {
                AttachPrimaryOutput::OutputLog
            } else {
                AttachPrimaryOutput::StdoutLog
            }
        } else {
            AttachPrimaryOutput::OutputLog
        };
    }
    if routes_session_output_to_output_log(metadata) {
        AttachPrimaryOutput::OutputLog
    } else {
        AttachPrimaryOutput::StdoutLog
    }
}

fn metadata_fragment_value(contents: &str, key: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let line = line.trim();
        let (fragment_key, fragment_value) = line.split_once('=')?;
        if fragment_key.trim() != key {
            return None;
        }
        let value = fragment_value.trim();
        let value = value.strip_prefix('"')?;
        let (value, _) = value.split_once('"')?;
        Some(value.to_string())
    })
}

fn attach_primary_output_from_metadata_fragment(
    contents: &str,
    output_log_exists: bool,
    session_active: bool,
) -> Option<AttachPrimaryOutput> {
    let runtime_binary = metadata_fragment_value(contents, "runtime_binary");
    let tool = metadata_fragment_value(contents, "tool").or_else(|| {
        let binary = runtime_binary.as_deref()?;
        let file_name = runtime_binary_file_name(binary).unwrap_or(binary);
        if file_name == "codex" || runtime_binary_indicates_codex_acp_file_name(file_name) {
            return Some("codex".to_string());
        }
        file_name
            .strip_suffix("-acp")
            .map(std::string::ToString::to_string)
    })?;
    let metadata = csa_session::metadata::SessionMetadata {
        tool,
        tool_locked: true,
        runtime_binary,
    };
    Some(attach_primary_output_from_metadata(
        &metadata,
        output_log_exists,
        session_active,
    ))
}

fn file_has_content(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.len() > 0)
        .unwrap_or(false)
}

fn file_modified_within(path: &Path, window: std::time::Duration) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified_at) = metadata.modified() else {
        return false;
    };
    modified_at
        .elapsed()
        .map(|elapsed| elapsed <= window)
        .unwrap_or(false)
}

fn stdout_log_looks_live(stdout_log: &Path) -> bool {
    file_has_content(stdout_log) && file_modified_within(stdout_log, ATTACH_STDOUT_LIVE_WINDOW)
}

fn attach_output_fallback(
    output_log: &Path,
    stdout_log: &Path,
    session_active: bool,
) -> AttachPrimaryOutput {
    let output_log_exists = output_log.is_file();
    if output_log_exists {
        AttachPrimaryOutput::OutputLog
    } else if !session_active || stdout_log_looks_live(stdout_log) {
        AttachPrimaryOutput::StdoutLog
    } else {
        AttachPrimaryOutput::AwaitMetadata
    }
}

fn should_fallback_attach_to_stdout(
    session_dir: &Path,
    stdout_log: &Path,
    output_log: &Path,
    elapsed: std::time::Duration,
) -> bool {
    elapsed >= ATTACH_METADATA_STDOUT_GRACE_WINDOW
        && session_has_terminal_process(session_dir)
        && !output_log.is_file()
        && file_has_content(stdout_log)
}

pub(super) fn attach_primary_output_for_session(session_dir: &Path) -> AttachPrimaryOutput {
    let output_log = session_dir.join("output.log");
    let stdout_log = session_dir.join("stdout.log");
    let session_active = session_has_terminal_process(session_dir);
    let metadata_path = session_dir.join(csa_session::metadata::METADATA_FILE_NAME);
    let Ok(contents) = fs::read_to_string(metadata_path) else {
        return attach_output_fallback(&output_log, &stdout_log, session_active);
    };
    let Ok(metadata) = toml::from_str::<csa_session::metadata::SessionMetadata>(&contents) else {
        return attach_primary_output_from_metadata_fragment(
            &contents,
            output_log.is_file(),
            session_active,
        )
        .unwrap_or_else(|| attach_output_fallback(&output_log, &stdout_log, session_active));
    };
    attach_primary_output_from_metadata(&metadata, output_log.is_file(), session_active)
}

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
    let mut unresolved_metadata_poll_interval = ATTACH_ROUTE_POLL_INTERVAL;

    loop {
        let primary_output = attach_primary_output_for_session(session_dir);
        let candidate = match primary_output {
            AttachPrimaryOutput::StdoutLog => stdout_path,
            AttachPrimaryOutput::OutputLog => output_path,
            AttachPrimaryOutput::AwaitMetadata => {
                if should_fallback_attach_to_stdout(
                    session_dir,
                    stdout_path,
                    output_path,
                    elapsed(),
                ) {
                    return Ok(Some(stdout_path.to_path_buf()));
                }
                if !session_has_terminal_process(session_dir) {
                    return Ok(None);
                }
                // Keep polling until metadata becomes readable or the session
                // exits; a transient metadata write failure must not force a
                // 30s attach failure while route selection is still unknown.
                sleep(unresolved_metadata_poll_interval);
                unresolved_metadata_poll_interval = std::cmp::min(
                    unresolved_metadata_poll_interval
                        .checked_mul(2)
                        .unwrap_or(ATTACH_METADATA_RETRY_MAX_INTERVAL),
                    ATTACH_METADATA_RETRY_MAX_INTERVAL,
                );
                continue;
            }
        };
        unresolved_metadata_poll_interval = ATTACH_ROUTE_POLL_INTERVAL;

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
