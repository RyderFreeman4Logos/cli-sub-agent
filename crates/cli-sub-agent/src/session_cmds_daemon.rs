//! Daemon-specific session commands extracted from `session_cmds.rs`.

use std::borrow::Cow;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use csa_session::state::ReviewSessionMeta;
use csa_session::{get_session_dir, load_output_index};
use serde::{Deserialize, Serialize};

#[path = "session_cmds_daemon_attach.rs"]
mod attach;

#[cfg(test)]
use attach::{
    ATTACH_METADATA_STDOUT_GRACE_WINDOW, attach_primary_output_for_session,
    attach_primary_output_from_metadata,
};
use attach::{resolve_attach_terminal_exit, wait_for_attach_live_output_path};

#[path = "session_cmds_daemon_wait.rs"]
mod wait;

use crate::session_cmds::resolve_session_prefix_with_global_fallback;

const DAEMON_SESSION_DIR_ENV: &str = "CSA_DAEMON_SESSION_DIR";
const DAEMON_PROJECT_ROOT_ENV: &str = "CSA_DAEMON_PROJECT_ROOT";
const DAEMON_COMPLETION_FILE: &str = "daemon-completion.toml";
const POST_REVIEW_PR_BOT_CMD: &str = "csa plan run --sa-mode true --pattern pr-bot";
pub(crate) use wait::handle_session_wait_with_memory_warn;
#[cfg(test)]
pub(crate) use wait::{
    SESSION_WAIT_MEMORY_WARN_EXIT_CODE, WaitBehavior, WaitLoopTiming, WaitReconciliationOutcome,
    handle_session_wait, handle_session_wait_with_hooks,
    handle_session_wait_with_hooks_and_sampler, synthesized_wait_next_step,
    try_acquire_session_wait_lock,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachPrimaryOutput {
    StdoutLog,
    OutputLog,
    AwaitMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DaemonCompletionPacket {
    exit_code: i32,
    status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UnpushedCommitsRecoveryPacket {
    recovery_command: String,
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

fn session_log_has_content(session_dir: &Path, log_name: &str) -> bool {
    fs::metadata(session_dir.join(log_name))
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false)
}

fn visible_output_logs_have_content(session_dir: &Path, output_log_visible: bool) -> bool {
    session_log_has_content(session_dir, "stdout.log")
        || (output_log_visible && session_log_has_content(session_dir, "output.log"))
}

fn failure_summary_for_empty_output(
    session_dir: &Path,
    output_streamed: bool,
    output_log_visible: bool,
) -> Option<String> {
    if output_streamed || visible_output_logs_have_content(session_dir, output_log_visible) {
        return None;
    }

    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    let contents = fs::read_to_string(result_path).ok()?;
    let result = toml::from_str::<csa_session::result::SessionResult>(&contents).ok()?;
    if result.status != "failure" || result.exit_code == 0 || result.summary.trim().is_empty() {
        return None;
    }

    Some(result.summary)
}

fn emit_failure_summary_for_empty_output(
    session_dir: &Path,
    output_streamed: bool,
    output_log_visible: bool,
) {
    let Some(summary) =
        failure_summary_for_empty_output(session_dir, output_streamed, output_log_visible)
    else {
        return;
    };
    eprintln!("{summary}");
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

/// Attach to a running daemon session, tailing the primary output channel and optional stderr.
pub(crate) fn handle_session_attach(
    session: String,
    show_stderr: bool,
    cd: Option<String>,
) -> Result<i32> {
    handle_session_attach_with_prompt(session, show_stderr, cd, None, None, None)
}

pub(crate) fn handle_session_attach_with_prompt(
    session: String,
    show_stderr: bool,
    cd: Option<String>,
    prompt: Option<String>,
    prompt_flag: Option<String>,
    prompt_file: Option<PathBuf>,
) -> Result<i32> {
    let caller_project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_global_fallback(&caller_project_root, &session)?;
    let project_root = resolved
        .foreign_project_root
        .clone()
        .unwrap_or_else(|| caller_project_root.clone());
    let session_dir = resolved.sessions_dir.join(&resolved.session_id);

    let prompt = resolve_attach_prompt(prompt, prompt_flag, prompt_file.as_deref())?;
    if let Some(prompt) = prompt {
        reactivate_session_with_prompt(&project_root, &session_dir, &resolved.session_id, &prompt)?;
    }

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
        return resolve_attach_terminal_exit(
            &project_root,
            &session_dir,
            &resolved.session_id,
            false,
            false,
        );
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
    let mut streamed_output = false;

    loop {
        let mut any_output = false;

        let n = live_stdout_file.read(&mut buf)?;
        if n > 0 {
            std::io::stdout().write_all(&buf[..n])?;
            std::io::stdout().flush()?;
            any_output = true;
            streamed_output = true;
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
                streamed_output = true;
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
                streamed_output = true;
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
                        streamed_output = true;
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
                    streamed_output = true;
                }
                std::io::stderr().flush()?;
            }
            emit_failure_summary_for_empty_output(
                &session_dir,
                streamed_output,
                live_streams_output_log,
            );
            // Return the session's exit code from result.toml.
            return Ok(completion.exit_code);
        }

        if !session_has_terminal_process(&session_dir) {
            return resolve_attach_terminal_exit(
                &project_root,
                &session_dir,
                &resolved.session_id,
                streamed_output,
                live_streams_output_log,
            );
        }

        if !any_output {
            std::thread::sleep(poll_interval);
        }
    }
}

fn resolve_attach_prompt(
    prompt: Option<String>,
    prompt_flag: Option<String>,
    prompt_file: Option<&Path>,
) -> Result<Option<String>> {
    let prompt = crate::run_helpers::resolve_positional_stdin_sentinel(prompt)?.or(prompt_flag);
    if let Some(path) = prompt_file {
        return crate::run_helpers::resolve_prompt_with_file(prompt, Some(path)).map(Some);
    }
    Ok(prompt)
}

fn reactivate_session_with_prompt(
    project_root: &Path,
    session_dir: &Path,
    session_id: &str,
    prompt: &str,
) -> Result<()> {
    let metadata = csa_session::load_metadata(project_root, session_id)?
        .ok_or_else(|| anyhow::anyhow!("session {session_id} is missing metadata.toml"))?;
    let session = csa_session::load_session(project_root, session_id)?;
    let actual_project_root = PathBuf::from(&session.project_path);

    if metadata.tool != "claude-code" {
        anyhow::bail!(
            "session attach --prompt only supports claude-code sessions; session {session_id} uses {}",
            metadata.tool
        );
    }
    if session.phase == csa_session::SessionPhase::Retired {
        anyhow::bail!("session {session_id} is retired and cannot be resumed");
    }
    if session.phase == csa_session::SessionPhase::Active
        && session_has_terminal_process(session_dir)
    {
        anyhow::bail!(
            "session {session_id} is already active with a running process; attach without --prompt or wait for it to finish"
        );
    }

    let provider_session_id =
        csa_session::resolve_resume_session(project_root, session_id, "claude-code")?
            .provider_session_id;
    if provider_session_id.is_none() {
        anyhow::bail!(
            "session {session_id} has no claude-code provider session ID recorded; cannot resume it with --prompt"
        );
    }

    // Clean old artifacts BEFORE spawn to avoid racing with the child process.
    // The user explicitly requested a resume — old data is intentionally superseded.
    // A spawn failure after cleanup is acceptable (user can retry; old run was
    // already marked for replacement by their --prompt invocation).
    clear_attach_reactivation_artifacts(session_dir)?;

    let prompt_path = persist_attach_prompt_file(session_dir, prompt)?;
    spawn_attach_resume_daemon(
        session_dir,
        session_id,
        &actual_project_root,
        &metadata.tool,
        &prompt_path,
    )
}

fn clear_attach_reactivation_artifacts(session_dir: &Path) -> Result<()> {
    let output_dir = session_dir.join("output");
    let indexed_paths = load_output_index(session_dir)
        .ok()
        .flatten()
        .map(|index| {
            index
                .sections
                .into_iter()
                .filter_map(|section| section.file_path)
                .filter(|path| output_relative_path_is_safe(path))
                .map(|path| output_dir.join(path))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    for path in indexed_paths {
        remove_file_if_exists(&path)?;
    }

    // Cleanup runs BEFORE spawn, so all prior-run files are safe to remove.
    // The daemon will create fresh stdout.log/stderr.log/output.log at spawn.
    for path in [
        session_dir.join("daemon-completion.toml"),
        session_dir.join("result.toml"),
        csa_session::contract_result_path(session_dir),
        csa_session::legacy_user_result_path(session_dir),
        session_dir.join("stdout.log"),
        session_dir.join("stderr.log"),
        session_dir.join("output.log"),
        session_dir.join("output.log.rotated"),
        output_dir.join("index.toml"),
        output_dir.join("acp-events.jsonl"),
    ] {
        remove_file_if_exists(&path)?;
    }

    Ok(())
}

fn output_relative_path_is_safe(path: &str) -> bool {
    let path = Path::new(path);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to remove {}", path.display())),
    }
}

fn persist_attach_prompt_file(session_dir: &Path, prompt: &str) -> Result<PathBuf> {
    let input_dir = session_dir.join("input");
    fs::create_dir_all(&input_dir)
        .with_context(|| format!("failed to create {}", input_dir.display()))?;
    let prompt_path = input_dir.join("attach-prompt.txt");
    fs::write(&prompt_path, prompt)
        .with_context(|| format!("failed to write {}", prompt_path.display()))?;
    Ok(prompt_path)
}

fn spawn_attach_resume_daemon(
    session_dir: &Path,
    session_id: &str,
    project_root: &Path,
    tool: &str,
    prompt_path: &Path,
) -> Result<()> {
    let csa_binary = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("csa"));
    let env = std::collections::HashMap::from([
        ("CSA_DAEMON_SESSION_ID".to_string(), session_id.to_string()),
        (
            "CSA_DAEMON_SESSION_DIR".to_string(),
            session_dir.display().to_string(),
        ),
        (
            "CSA_DAEMON_PROJECT_ROOT".to_string(),
            project_root.display().to_string(),
        ),
    ]);
    let args = vec![
        "--sa-mode".to_string(),
        "false".to_string(),
        "--tool".to_string(),
        tool.to_string(),
        "--force".to_string(),
        "--session".to_string(),
        session_id.to_string(),
        "--cd".to_string(),
        project_root.display().to_string(),
        "--prompt-file".to_string(),
        prompt_path.display().to_string(),
    ];

    csa_process::daemon::spawn_daemon(csa_process::daemon::DaemonSpawnConfig {
        session_id: session_id.to_string(),
        session_dir: session_dir.to_path_buf(),
        csa_binary,
        subcommand: "run".to_string(),
        args,
        env,
    })
    .map(|_| ())
    .context("failed to spawn resumed daemon session")
}

/// Kill a session with SIGTERM, then SIGKILL after a 5-second grace period if needed.
///
/// Resolution order for the target PID (#1118 part C):
/// 1. Live daemon leader from `daemon.pid` (matches PID + start-time).
/// 2. Stale `daemon.pid` → bail (refuses to signal a potentially reused PID).
/// 3. Inline (non-daemon) tool process from session lock files in `locks/`.
///    The lock-holding tool process is its own session leader (spawned via
///    `setsid`), so `kill(-pid, SIG)` propagates to the whole process group
///    just like the daemon path.
pub(crate) fn handle_session_kill(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_global_fallback(&project_root, &session)?;
    let session_dir = resolved.sessions_dir.join(&resolved.session_id);

    let (pid, kind) = if let Some(pid) =
        csa_process::ToolLiveness::daemon_pid_for_signal(&session_dir)
    {
        (pid, "daemon")
    } else if let Some(stale_pid) = read_daemon_pid(&session_dir) {
        anyhow::bail!(
            "Stored daemon PID {} for session {} no longer matches a live session process; refusing to signal a potentially reused PID",
            stale_pid,
            resolved.session_id,
        );
    } else if let Some(pid) = csa_process::ToolLiveness::live_process_pid(&session_dir) {
        (pid, "inline")
    } else {
        anyhow::bail!(
            "No live PID found for session {} — session has neither a daemon.pid file nor a live tool lock file in {}/locks/",
            resolved.session_id,
            session_dir.display(),
        );
    };

    if pid <= 1 {
        anyhow::bail!(
            "Refusing to kill PID {} — invalid PID (would target init or caller's process group)",
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
        "Sending SIGTERM to {} session {} (PID {})...",
        kind, resolved.session_id, pid,
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
            pid
        );
    }
    eprintln!("Session {} killed", resolved.session_id);
    Ok(())
}

#[cfg(test)]
#[path = "session_cmds_daemon_attach_proptest.rs"]
mod session_cmds_daemon_attach_proptest;
#[cfg(test)]
#[path = "session_cmds_daemon_routing_proptest.rs"]
mod session_cmds_daemon_routing_proptest;
#[cfg(test)]
#[path = "session_cmds_daemon_tests.rs"]
mod tests;
