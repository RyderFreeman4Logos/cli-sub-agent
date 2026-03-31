//! Daemon-specific session commands: wait and attach.
//!
//! Extracted from session_cmds.rs to stay under the monolith file limit.

use std::fs;
use std::io::{Read, Write};

use anyhow::Result;
use csa_session::get_session_dir;

use crate::session_cmds::resolve_session_prefix_with_fallback;

/// Wait for a daemon session to complete by polling for result.toml.
///
/// Exits 0 when result.toml appears (prints stdout.log), exits 124 on timeout.
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

    loop {
        if result_path.exists() {
            let stdout_log = session_dir.join("stdout.log");
            if stdout_log.is_file() {
                let content = fs::read_to_string(&stdout_log)?;
                if !content.is_empty() {
                    print!("{content}");
                }
            }
            return Ok(0);
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

    let mut stdout_file = std::fs::File::open(&stdout_path)?;
    let mut stderr_file = if show_stderr && stderr_path.exists() {
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

        if let Some(ref mut f) = stderr_file {
            let n = f.read(&mut buf)?;
            if n > 0 {
                std::io::stderr().write_all(&buf[..n])?;
                std::io::stderr().flush()?;
                any_output = true;
            }
        }

        if result_path.exists() {
            loop {
                let n = stdout_file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                std::io::stdout().write_all(&buf[..n])?;
            }
            std::io::stdout().flush()?;
            return Ok(0);
        }

        if !any_output {
            std::thread::sleep(poll_interval);
        }
    }
}
