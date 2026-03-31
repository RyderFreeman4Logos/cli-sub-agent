//! Daemon-specific session commands: wait and attach.
//!
//! Extracted from session_cmds.rs to stay under the monolith file limit.

use std::fs;
use std::io::{Read, Write};

use anyhow::Result;
use csa_session::get_session_dir;

use crate::session_cmds::resolve_session_prefix_with_fallback;

/// Check whether a daemon PID is still running.
fn is_pid_alive(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) is a standard POSIX liveness probe.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Read the daemon PID from the session directory's spool metadata.
/// Returns None if the PID file doesn't exist or can't be parsed.
fn read_daemon_pid(session_dir: &std::path::Path) -> Option<u32> {
    // The daemon parent writes the PID to stdout.log's parent dir as pid.txt
    // (not yet implemented); fall back to parsing stderr for the RPJ directive.
    let stderr_path = session_dir.join("stderr.log");
    if let Ok(content) = fs::read_to_string(&stderr_path) {
        // Parse "CSA:SESSION_STARTED id=... pid=<N> ..."
        if let Some(pid_start) = content.find("pid=") {
            let rest = &content[pid_start + 4..];
            let pid_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            return pid_str.parse().ok();
        }
    }
    None
}

/// Wait for a daemon session to complete by polling for result.toml.
///
/// Exits 0 when result.toml appears (streams stdout.log), exits 124 on timeout,
/// exits 1 if the daemon process died without producing a result.
pub(crate) fn handle_session_wait(
    session: String,
    timeout_secs: u64,
    cd: Option<String>,
) -> Result<i32> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let session_dir = get_session_dir(&project_root, &resolved.session_id)?;
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);

    let start = std::time::Instant::now();
    let poll_interval = std::time::Duration::from_secs(1);
    let daemon_pid = read_daemon_pid(&session_dir);

    loop {
        if result_path.exists() {
            // Stream stdout.log to avoid OOM on large daemon output.
            let stdout_log = session_dir.join("stdout.log");
            if stdout_log.is_file() {
                let mut f = std::fs::File::open(&stdout_log)?;
                std::io::copy(&mut f, &mut std::io::stdout().lock())?;
            }
            return Ok(0);
        }

        // Detect dead daemon: PID gone but no result.toml.
        if let Some(pid) = daemon_pid
            && !is_pid_alive(pid)
        {
            eprintln!(
                "Daemon process {} exited without producing result.toml",
                pid,
            );
            return Ok(1);
        }

        if timeout_secs > 0 && start.elapsed().as_secs() >= timeout_secs {
            eprintln!(
                "Timeout: session {} did not complete within {}s",
                resolved.session_id, timeout_secs
            );
            return Ok(124);
        }

        std::thread::sleep(poll_interval);
    }
}

/// Attach to a running daemon session: tail stdout.log (and optionally
/// stderr.log) in real time until the session completes.
pub(crate) fn handle_session_attach(
    session: String,
    show_stderr: bool,
    cd: Option<String>,
) -> Result<i32> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let session_dir = get_session_dir(&project_root, &resolved.session_id)?;
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);

    let stdout_path = session_dir.join("stdout.log");
    let stderr_path = session_dir.join("stderr.log");

    // Wait for the spool file to appear (daemon may still be starting).
    let start = std::time::Instant::now();
    while !stdout_path.exists() {
        if start.elapsed().as_secs() > 30 {
            anyhow::bail!(
                "stdout.log not found after 30s — session {} may not be a daemon session",
                resolved.session_id,
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    let daemon_pid = read_daemon_pid(&session_dir);
    let mut stdout_file = std::fs::File::open(&stdout_path)?;
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

        let n = stdout_file.read(&mut buf)?;
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

        if result_path.exists() {
            // Drain remaining stdout.
            loop {
                let n = stdout_file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                std::io::stdout().write_all(&buf[..n])?;
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
            let exit_code = fs::read_to_string(&result_path)
                .ok()
                .and_then(|s| toml::from_str::<csa_session::result::SessionResult>(&s).ok())
                .map(|r| r.exit_code)
                .unwrap_or(0);
            return Ok(exit_code);
        }

        // Detect dead daemon: PID gone but no result.toml.
        if let Some(pid) = daemon_pid
            && !is_pid_alive(pid)
        {
            eprintln!(
                "Daemon process {} exited without producing result.toml",
                pid,
            );
            return Ok(1);
        }

        if !any_output {
            std::thread::sleep(poll_interval);
        }
    }
}
