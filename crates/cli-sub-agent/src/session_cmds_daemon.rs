//! Daemon-specific session commands extracted from `session_cmds.rs`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use csa_executor::{TransportFactory, TransportMode};
use csa_session::get_session_dir;
use csa_session::state::ReviewSessionMeta;
use serde::{Deserialize, Serialize};

#[path = "session_cmds_daemon_attach.rs"]
mod attach;

use attach::{resolve_attach_terminal_exit, wait_for_attach_live_output_path};

use crate::session_cmds::resolve_session_prefix_with_global_fallback;

const DAEMON_SESSION_DIR_ENV: &str = "CSA_DAEMON_SESSION_DIR";
const DAEMON_PROJECT_ROOT_ENV: &str = "CSA_DAEMON_PROJECT_ROOT";
const DAEMON_COMPLETION_FILE: &str = "daemon-completion.toml";
const POST_REVIEW_PR_BOT_CMD: &str = "csa plan run --sa-mode true --pattern pr-bot";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachPrimaryOutput {
    StdoutLog,
    OutputLog,
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DaemonCompletionPacket {
    exit_code: i32,
    status: String,
}

impl DaemonCompletionPacket {
    fn from_exit_code(exit_code: i32) -> Self {
        Self {
            exit_code,
            status: csa_session::SessionResult::status_from_exit_code(exit_code),
        }
    }
}

/// Check whether a daemon PID is still running.
fn is_pid_alive(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) is a standard POSIX liveness probe.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn session_has_terminal_process(session_dir: &Path) -> bool {
    csa_process::ToolLiveness::has_live_process(session_dir)
        || csa_process::ToolLiveness::daemon_pid_is_alive(session_dir)
}

fn routes_session_output_to_output_log(metadata: &csa_session::metadata::SessionMetadata) -> bool {
    if metadata.runtime_binary.as_deref() == Some("codex-acp") {
        return true;
    }
    matches!(
        TransportFactory::mode_for_tool(&metadata.tool),
        TransportMode::Acp
    )
}

fn attach_output_fallback(
    output_log_exists: bool,
    stdout_log_exists: bool,
    session_active: bool,
) -> AttachPrimaryOutput {
    if session_active && stdout_log_exists && !output_log_exists {
        AttachPrimaryOutput::Pending
    } else if output_log_exists {
        AttachPrimaryOutput::OutputLog
    } else {
        AttachPrimaryOutput::StdoutLog
    }
}

fn attach_primary_output_for_session(session_dir: &Path) -> AttachPrimaryOutput {
    let output_log = session_dir.join("output.log");
    let stdout_log = session_dir.join("stdout.log");
    let output_log_exists = output_log.is_file();
    let stdout_log_exists = stdout_log.is_file();
    let session_active = session_has_terminal_process(session_dir);
    let metadata_path = session_dir.join(csa_session::metadata::METADATA_FILE_NAME);
    let Ok(contents) = fs::read_to_string(metadata_path) else {
        return attach_output_fallback(output_log_exists, stdout_log_exists, session_active);
    };
    let Ok(metadata) = toml::from_str::<csa_session::metadata::SessionMetadata>(&contents) else {
        return attach_output_fallback(output_log_exists, stdout_log_exists, session_active);
    };
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
    if routes_session_output_to_output_log(&metadata) {
        AttachPrimaryOutput::OutputLog
    } else {
        AttachPrimaryOutput::StdoutLog
    }
}

/// Read the daemon PID from `daemon.pid`, falling back to legacy stderr directives.
fn read_daemon_pid(session_dir: &std::path::Path) -> Option<u32> {
    // Primary: daemon.pid file (written by spawn_daemon since v0.1.198).
    let pid_path = session_dir.join("daemon.pid");
    if let Ok(content) = fs::read_to_string(&pid_path)
        && let Some(pid_str) = content.split_whitespace().next()
        && let Ok(pid) = pid_str.parse()
    {
        return Some(pid);
    }
    // Fallback: parse stderr for the RPJ directive (legacy sessions).
    let stderr_path = session_dir.join("stderr.log");
    if let Ok(content) = fs::read_to_string(&stderr_path)
        && let Some(pid_start) = content.find("pid=")
    {
        let rest = &content[pid_start + 4..];
        let pid_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        return pid_str.parse().ok();
    }
    None
}

pub(crate) fn seed_daemon_session_env(session_id: &str, cd: Option<&str>) {
    let project_root = match crate::pipeline::determine_project_root(cd) {
        Ok(root) => root,
        Err(_) => return,
    };
    let session_dir = match get_session_dir(&project_root, session_id) {
        Ok(dir) => dir,
        Err(_) => return,
    };

    // SAFETY: daemon child sets process-scoped env before async worker tasks rely on it.
    unsafe {
        std::env::set_var(DAEMON_PROJECT_ROOT_ENV, &project_root);
        std::env::set_var(DAEMON_SESSION_DIR_ENV, &session_dir);
    }
}

pub(crate) fn persist_daemon_completion_from_env(exit_code: i32) {
    let session_dir = resolve_daemon_session_dir_from_env();
    let Some(session_dir) = session_dir else {
        return;
    };
    let _ = persist_daemon_completion(&session_dir, exit_code);
}

/// Wait for a daemon session to reach a terminal result and daemon exit.
/// Exits 0 on completion, 124 on timeout, and 1 if the daemon dies without a result.
pub(crate) fn handle_session_wait(
    session: String,
    cd: Option<String>,
    wait_timeout_secs: u64,
) -> Result<i32> {
    handle_session_wait_with_hooks(
        session,
        cd,
        wait_timeout_secs,
        |project_root, session_id, trigger| {
            let reconciled = crate::session_cmds::ensure_terminal_result_for_dead_active_session(
                project_root,
                session_id,
                trigger,
            )?;
            Ok(WaitReconciliationOutcome {
                result_became_available: reconciled.result_became_available(),
                synthetic: reconciled.synthesized_failure(),
            })
        },
        emit_wait_completion_signal,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitReconciliationOutcome {
    pub(crate) result_became_available: bool,
    pub(crate) synthetic: bool,
}

pub(crate) fn handle_session_wait_with_hooks<R, E>(
    session: String,
    cd: Option<String>,
    wait_timeout_secs: u64,
    mut reconcile_dead_active_session: R,
    mut emit_completion_signal: E,
) -> Result<i32>
where
    R: FnMut(&Path, &str, &str) -> Result<WaitReconciliationOutcome>,
    E: FnMut(&str, &str, i32, bool, bool),
{
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_global_fallback(&project_root, &session)?;
    // For cross-project sessions, derive session_dir from the resolved sessions_dir
    let session_dir = resolved.sessions_dir.join(&resolved.session_id);

    // Use the foreign project root for cross-project sessions, local otherwise.
    let effective_root = resolved
        .foreign_project_root
        .as_deref()
        .unwrap_or(&project_root);
    let is_cross_project = resolved.foreign_project_root.is_some();

    let start = std::time::Instant::now();
    let poll_interval = std::time::Duration::from_secs(1);

    loop {
        if let Some(completion) = load_daemon_completion_packet(&session_dir)?
            && !session_has_terminal_process(&session_dir)
        {
            let _ = refresh_result_for_wait(
                effective_root,
                &resolved.session_id,
                &session_dir,
                is_cross_project,
            );
            let streamed_output = stream_wait_output(&session_dir)?;
            emit_wait_next_step_if_needed(&session_dir)?;
            emit_completion_signal(
                &resolved.session_id,
                &completion.status,
                completion.exit_code,
                false,
                !streamed_output,
            );
            return Ok(completion.exit_code);
        }

        if let Some(result) = load_completed_daemon_result_adaptive(
            effective_root,
            &resolved.session_id,
            &session_dir,
            is_cross_project,
        )? {
            let streamed_output = stream_wait_output(&session_dir)?;
            emit_wait_next_step_if_needed(&session_dir)?;
            emit_completion_signal(
                &resolved.session_id,
                &result.status,
                result.exit_code,
                false,
                !streamed_output,
            );
            return Ok(result.exit_code);
        }

        // Synthesize terminal result for dead Active sessions.
        let reconciled =
            reconcile_dead_active_session(effective_root, &resolved.session_id, "session wait")?;
        if reconciled.result_became_available
            && let Some(result) = load_completed_daemon_result_adaptive(
                effective_root,
                &resolved.session_id,
                &session_dir,
                is_cross_project,
            )?
        {
            let streamed_output = stream_wait_output(&session_dir)?;
            emit_wait_next_step_if_needed(&session_dir)?;
            emit_completion_signal(
                &resolved.session_id,
                &result.status,
                result.exit_code,
                reconciled.synthetic,
                !streamed_output,
            );
            if reconciled.synthetic && !streamed_output {
                eprintln!(
                    "Session {} reached a synthesized terminal result because no live daemon process remained.",
                    resolved.session_id,
                );
            }
            return Ok(result.exit_code);
        }

        if !session_has_terminal_process(&session_dir) {
            if let Some(result) = load_completed_daemon_result_adaptive(
                effective_root,
                &resolved.session_id,
                &session_dir,
                is_cross_project,
            )? {
                let streamed_output = stream_wait_output(&session_dir)?;
                emit_wait_next_step_if_needed(&session_dir)?;
                emit_completion_signal(
                    &resolved.session_id,
                    &result.status,
                    result.exit_code,
                    false,
                    !streamed_output,
                );
                return Ok(result.exit_code);
            }
            eprintln!(
                "Session {} has no live daemon process and no terminal result packet.",
                resolved.session_id,
            );
            eprintln!(
                "Run `csa session result --session {}` for diagnostics.",
                resolved.session_id
            );
            return Ok(1);
        }

        let elapsed = start.elapsed().as_secs();
        if elapsed >= wait_timeout_secs {
            eprintln!(
                "Timeout: session {} did not complete within {}s",
                resolved.session_id, wait_timeout_secs,
            );
            // Emit structured retry hint for orchestrators / agents.
            let cd_arg = cd
                .as_ref()
                .map(|path| format!(" --cd '{}'", path))
                .unwrap_or_default();
            eprintln!(
                "<!-- CSA:SESSION_WAIT_TIMEOUT session={} elapsed={}s cmd=\"csa session wait --session {}{}\" -->",
                resolved.session_id, elapsed, resolved.session_id, cd_arg,
            );
            eprintln!(
                "Hint: Call `csa session wait` again individually (not in a tight loop script). \
                 The {}s timeout is designed to let the calling agent generate tokens between waits, \
                 keeping its KV cache warm.",
                wait_timeout_secs,
            );
            eprintln!(
                "<!-- CSA:CALLER_HINT action=\"retry_wait\" \
                 rule=\"Process this output NOW and generate tokens to warm your KV cache, \
                 then call 'csa session wait --session {sid}{cd}' again in a NEW Bash call. \
                 NEVER batch multiple session waits in one Bash call.\" -->",
                sid = resolved.session_id,
                cd = cd
                    .as_ref()
                    .map(|p| format!(" --cd '{p}'"))
                    .unwrap_or_default(),
            );
            return Ok(124);
        }

        std::thread::sleep(poll_interval);
    }
}

fn stream_wait_output(session_dir: &std::path::Path) -> Result<bool> {
    let stdout_log = session_dir.join("stdout.log");
    if !stdout_log.is_file() {
        return Ok(false);
    }

    let mut file = std::fs::File::open(&stdout_log)?;
    let mut stdout = std::io::stdout().lock();
    let bytes = std::io::copy(&mut file, &mut stdout)?;
    stdout.flush()?;
    Ok(bytes > 0)
}

pub(crate) fn synthesized_wait_next_step(session_dir: &Path) -> Result<Option<String>> {
    let stdout_path = session_dir.join("stdout.log");
    if let Ok(stdout) = fs::read_to_string(&stdout_path)
        && csa_hooks::parse_next_step_directive(&stdout).is_some()
    {
        return Ok(None);
    }

    let review_meta_path = session_dir.join("review_meta.json");
    if !review_meta_path.is_file() {
        return Ok(None);
    }

    let review_meta: ReviewSessionMeta =
        serde_json::from_str(&fs::read_to_string(review_meta_path)?)?;
    if review_meta.decision != "pass" {
        return Ok(None);
    }
    if !(review_meta.scope.starts_with("base:") || review_meta.scope.starts_with("range:")) {
        return Ok(None);
    }

    Ok(Some(csa_hooks::format_next_step_directive(
        POST_REVIEW_PR_BOT_CMD,
        true,
    )))
}

fn emit_wait_next_step_if_needed(session_dir: &Path) -> Result<()> {
    if let Some(directive) = synthesized_wait_next_step(session_dir)? {
        println!("{directive}");
    }
    Ok(())
}

fn load_completed_daemon_result(
    project_root: &std::path::Path,
    session_id: &str,
    session_dir: &std::path::Path,
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

    if session_has_terminal_process(session_dir) {
        return Ok(None);
    }

    Ok(Some(result))
}

/// Refresh result via session_dir for cross-project sessions or via project_root otherwise.
fn refresh_result_for_wait(
    project_root: &std::path::Path,
    session_id: &str,
    session_dir: &std::path::Path,
    is_cross_project: bool,
) -> Result<Option<csa_session::SessionResult>> {
    if is_cross_project {
        crate::session_observability::refresh_and_repair_result_from_dir(session_dir)
    } else {
        crate::session_observability::refresh_and_repair_result(project_root, session_id)
    }
}

/// Load completed daemon result, adapting for cross-project sessions.
fn load_completed_daemon_result_adaptive(
    project_root: &std::path::Path,
    session_id: &str,
    session_dir: &std::path::Path,
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
        if session_has_terminal_process(session_dir) {
            return Ok(None);
        }
        Ok(Some(result))
    } else {
        load_completed_daemon_result(project_root, session_id, session_dir)
    }
}

fn load_daemon_completion_packet(session_dir: &Path) -> Result<Option<DaemonCompletionPacket>> {
    let path = daemon_completion_path(session_dir);
    if !path.is_file() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)?;
    let packet = toml::from_str(&content)?;
    Ok(Some(packet))
}

fn persist_daemon_completion(session_dir: &Path, exit_code: i32) -> Result<()> {
    let packet = DaemonCompletionPacket::from_exit_code(exit_code);
    let path = daemon_completion_path(session_dir);
    let temp_path = path.with_extension("toml.tmp");
    fs::write(&temp_path, toml::to_string_pretty(&packet)?)?;
    fs::rename(temp_path, path)?;
    Ok(())
}

fn daemon_completion_path(session_dir: &Path) -> PathBuf {
    session_dir.join(DAEMON_COMPLETION_FILE)
}

fn resolve_daemon_session_dir_from_env() -> Option<PathBuf> {
    if let Some(session_dir) = std::env::var_os(DAEMON_SESSION_DIR_ENV) {
        return Some(PathBuf::from(session_dir));
    }

    let session_id = std::env::var("CSA_DAEMON_SESSION_ID").ok()?;
    let project_root = std::env::var_os(DAEMON_PROJECT_ROOT_ENV)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())?;
    get_session_dir(&project_root, &session_id).ok()
}

fn emit_wait_completion_signal(
    session_id: &str,
    status: &str,
    exit_code: i32,
    synthetic: bool,
    _mirror_to_stdout: bool,
) {
    let signal = format!(
        "<!-- CSA:SESSION_WAIT_COMPLETED session={} status={} exit={} synthetic={} -->",
        session_id, status, exit_code, synthetic
    );
    println!("{signal}");
    eprintln!("{signal}");
    eprintln!(
        "<!-- CSA:CALLER_HINT action=\"next_session\" \
         rule=\"If you have more sessions to wait for, call each 'csa session wait' \
         in a SEPARATE Bash call. NEVER batch. \
         Generate tokens between waits to keep your KV cache warm.\" -->"
    );
}

/// Attach to a running daemon session, tailing the primary output channel and optional stderr.
pub(crate) fn handle_session_attach(
    session: String,
    show_stderr: bool,
    cd: Option<String>,
) -> Result<i32> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_global_fallback(&project_root, &session)?;
    let session_dir = resolved.sessions_dir.join(&resolved.session_id);

    let stdout_path = session_dir.join("stdout.log");
    let stderr_path = session_dir.join("stderr.log");
    let output_path = session_dir.join("output.log");
    let start = std::time::Instant::now();
    let live_stdout_path = wait_for_attach_live_output_path(
        &session_dir,
        &resolved.session_id,
        &stdout_path,
        &output_path,
        || start.elapsed(),
        std::thread::sleep,
    )?;
    let Some(live_stdout_path) = live_stdout_path else {
        return resolve_attach_terminal_exit(&project_root, &session_dir, &resolved.session_id);
    };

    let live_streams_output_log = live_stdout_path == output_path;
    let mut live_stdout_file = std::fs::File::open(&live_stdout_path)?;
    let mut completion_stdout_file: Option<std::fs::File> =
        if live_streams_output_log && stdout_path.exists() {
            Some(std::fs::File::open(&stdout_path)?)
        } else {
            None
        };
    // Lazy-open stderr: retry on each poll iteration if not yet available.
    let mut stderr_file: Option<std::fs::File> = if show_stderr && stderr_path.exists() {
        Some(std::fs::File::open(&stderr_path)?)
    } else {
        None
    };

    let mut buf = [0u8; 8192];
    let poll_interval = std::time::Duration::from_millis(100);

    loop {
        let mut any_output = false;

        let n = live_stdout_file.read(&mut buf)?;
        if n > 0 {
            std::io::stdout().write_all(&buf[..n])?;
            std::io::stdout().flush()?;
            any_output = true;
        }

        // Lazy-open stderr if it appeared after we started.
        if show_stderr && stderr_file.is_none() && stderr_path.exists() {
            stderr_file = std::fs::File::open(&stderr_path).ok();
        }
        if let Some(ref mut f) = stderr_file {
            let n = f.read(&mut buf)?;
            if n > 0 {
                std::io::stderr().write_all(&buf[..n])?;
                std::io::stderr().flush()?;
                any_output = true;
            }
        }

        if let Some(completion) = load_daemon_completion_packet(&session_dir)?
            && !session_has_terminal_process(&session_dir)
        {
            // Drain remaining stdout.
            loop {
                let n = live_stdout_file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                std::io::stdout().write_all(&buf[..n])?;
            }
            if live_streams_output_log {
                if completion_stdout_file.is_none() && stdout_path.exists() {
                    completion_stdout_file = Some(std::fs::File::open(&stdout_path)?);
                }
                if let Some(ref mut f) = completion_stdout_file {
                    loop {
                        let n = f.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        std::io::stdout().write_all(&buf[..n])?;
                    }
                }
            }
            std::io::stdout().flush()?;
            // Drain remaining stderr.
            if let Some(ref mut f) = stderr_file {
                loop {
                    let n = f.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    std::io::stderr().write_all(&buf[..n])?;
                }
                std::io::stderr().flush()?;
            }
            // Return the session's exit code from result.toml.
            return Ok(completion.exit_code);
        }

        if !session_has_terminal_process(&session_dir) {
            return resolve_attach_terminal_exit(&project_root, &session_dir, &resolved.session_id);
        }

        if !any_output {
            std::thread::sleep(poll_interval);
        }
    }
}

/// Kill a daemon session with SIGTERM, then SIGKILL after a 5-second grace period if needed.
pub(crate) fn handle_session_kill(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_global_fallback(&project_root, &session)?;
    let session_dir = resolved.sessions_dir.join(&resolved.session_id);

    let pid = if let Some(pid) = csa_process::ToolLiveness::daemon_pid_for_signal(&session_dir) {
        pid
    } else if let Some(stale_pid) = read_daemon_pid(&session_dir) {
        anyhow::bail!(
            "Stored daemon PID {} for session {} no longer matches a live session process; refusing to signal a potentially reused PID",
            stale_pid,
            resolved.session_id,
        );
    } else {
        anyhow::bail!(
            "No daemon PID found for session {} — may not be a daemon session",
            resolved.session_id,
        );
    };

    if pid <= 1 {
        anyhow::bail!(
            "Refusing to kill PID {} — invalid daemon PID (would target init or caller's process group)",
            pid,
        );
    }

    if !is_pid_alive(pid) {
        eprintln!(
            "Session {} (PID {}) is already dead",
            resolved.session_id, pid,
        );
        return Ok(());
    }

    // Send SIGTERM to the process group (negative PID).
    eprintln!(
        "Sending SIGTERM to session {} (PID {})...",
        resolved.session_id, pid,
    );
    // SAFETY: kill(-pid, SIGTERM) sends to the entire process group.
    let pgid = -(pid as libc::pid_t);
    let rc = unsafe { libc::kill(pgid, libc::SIGTERM) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        eprintln!("Warning: SIGTERM failed for PID {pid}: {err}");
    }

    // Grace period: wait up to 5 seconds for clean shutdown.
    for _ in 0..50 {
        if !is_pid_alive(pid) {
            eprintln!("Session {} terminated", resolved.session_id);
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // Force kill.
    eprintln!(
        "Session {} still alive after 5s, sending SIGKILL...",
        resolved.session_id,
    );
    // SAFETY: kill(-pid, SIGKILL) force-kills the entire process group.
    let rc = unsafe { libc::kill(pgid, libc::SIGKILL) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        eprintln!("Warning: SIGKILL failed for PID {pid}: {err}");
    }

    // Wait for reaping.
    std::thread::sleep(std::time::Duration::from_millis(500));
    if is_pid_alive(pid) {
        anyhow::bail!(
            "Failed to kill session {} (PID {})",
            resolved.session_id,
            pid,
        );
    }

    eprintln!("Session {} killed", resolved.session_id);
    Ok(())
}

#[cfg(test)]
#[path = "session_cmds_daemon_tests.rs"]
mod tests;
