use anyhow::{Context, Result};
use std::time::Duration;
use tokio::process::Command;

pub(crate) async fn terminate_child_process_group(
    child: &mut tokio::process::Child,
    termination_grace_period: Duration,
) -> Option<std::process::ExitStatus> {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // SAFETY: kill() is async-signal-safe; negative PID targets the process group.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            tokio::time::sleep(termination_grace_period).await;
            // Do not reap the leader before the final group signal: its unreaped
            // PID anchors the PGID even when it exited on SIGTERM.
            // SAFETY: kill() is async-signal-safe; negative PID targets the process group.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            return child.wait().await.ok();
        }
    }

    let _ = child.start_kill();
    child.wait().await.ok()
}

/// Check if a tool is installed by attempting to locate it.
///
/// Uses `which` command on Unix systems.
pub async fn check_tool_installed(executable: &str) -> Result<()> {
    let output = Command::new("which")
        .arg(executable)
        .output()
        .await
        .context("Failed to execute 'which' command")?;

    if !output.status.success() {
        anyhow::bail!("Tool '{executable}' is not installed or not in PATH");
    }

    Ok(())
}
