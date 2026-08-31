use anyhow::{Context, Result};
use std::{process::ExitStatus, time::Duration};
use tokio::process::Command;

#[cfg(test)]
use std::sync::atomic::{AtomicI32, Ordering};

const CHILD_REAP_TIMEOUT: Duration = Duration::from_secs(1);

#[cfg(test)]
static FORCE_GROUP_CLEANUP_TIMEOUT_FOR: AtomicI32 = AtomicI32::new(0);

#[derive(Debug)]
pub enum ChildWaitState {
    Running,
    Exited(ExitStatus),
}

/// Inspect a child without releasing its PID/process-group identity on Unix.
pub fn inspect_child_without_reaping(
    child: &mut tokio::process::Child,
) -> std::io::Result<ChildWaitState> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        let Some(pid) = child.id() else {
            return child.try_wait()?.map_or_else(
                || Err(std::io::Error::other("child PID unavailable before exit")),
                |status| Ok(ChildWaitState::Exited(status)),
            );
        };
        loop {
            // SAFETY: zero is the documented no-state-change sentinel for
            // siginfo_t returned by waitid with WNOHANG.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            match waitid_without_reaping(pid as libc::id_t, &mut info) {
                Ok(()) => {
                    // SAFETY: waitid initialized info; si_pid == 0 means no state change.
                    if unsafe { info.si_pid() } == 0 {
                        return Ok(ChildWaitState::Running);
                    }
                    // SAFETY: this is a SIGCHLD result from waitid(P_PID, ...).
                    let status = unsafe { info.si_status() };
                    let raw_status = match info.si_code {
                        libc::CLD_EXITED => status << 8,
                        libc::CLD_KILLED => status,
                        libc::CLD_DUMPED => status | 0x80,
                        code => {
                            return Err(std::io::Error::other(format!(
                                "unexpected waitid exit code {code}"
                            )));
                        }
                    };
                    return Ok(ChildWaitState::Exited(ExitStatus::from_raw(raw_status)));
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }

    #[cfg(not(unix))]
    {
        Ok(match child.try_wait()? {
            Some(status) => ChildWaitState::Exited(status),
            None => ChildWaitState::Running,
        })
    }
}

#[cfg(unix)]
pub(crate) fn waitid_without_reaping(
    id: libc::id_t,
    info: &mut libc::siginfo_t,
) -> std::io::Result<()> {
    // SAFETY: info points to writable siginfo_t storage. WNOWAIT leaves an
    // exited child waitable, preserving its PID as the process-group anchor.
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            id,
            info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if rc == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn signal_process_group(process_group: i32, signal: libc::c_int) -> Result<()> {
    loop {
        // SAFETY: the negative PID targets the still-owned child's process group.
        let rc = unsafe { libc::kill(-process_group, signal) };
        if rc == 0 {
            return Ok(());
        }

        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error)
                .with_context(|| format!("failed to signal process group {process_group}"));
        }
    }
}

async fn wait_for_child_exit(child: &mut tokio::process::Child) -> Result<ExitStatus> {
    tokio::time::timeout(CHILD_REAP_TIMEOUT, child.wait())
        .await
        .context("timed out waiting for child process to exit")?
        .context("failed to wait for child process")
}

#[cfg(target_os = "linux")]
async fn wait_for_process_group_termination(process_group: i32) -> Result<()> {
    #[cfg(test)]
    if FORCE_GROUP_CLEANUP_TIMEOUT_FOR.swap(0, Ordering::SeqCst) == process_group {
        anyhow::bail!("forced process-group cleanup timeout");
    }

    let deadline = tokio::time::Instant::now() + CHILD_REAP_TIMEOUT;
    loop {
        match crate::process_activity::process_group_has_live_members(process_group) {
            Ok(false) => return Ok(()),
            Ok(true) => {
                // A descendant can fork between the initial group kill and
                // this observation. Keep the kill fenced to the owned PGID
                // until the scanner observes no live members.
                signal_process_group(process_group, libc::SIGKILL)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                return Err(error).context(format!(
                    "failed to inspect child process group {process_group}"
                ));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for child process group {process_group} to terminate");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub async fn terminate_child_process_group(
    child: &mut tokio::process::Child,
    termination_grace_period: Duration,
) -> Result<std::process::ExitStatus> {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            let process_group = i32::try_from(pid).context("child PID exceeds pid_t range")?;
            signal_process_group(process_group, libc::SIGTERM)?;
            if !termination_grace_period.is_zero() {
                tokio::time::sleep(termination_grace_period).await;
            }
            // Do not reap the leader before the final group signal: its unreaped
            // PID anchors the PGID even when it exited on SIGTERM.
            signal_process_group(process_group, libc::SIGKILL)?;
            #[cfg(target_os = "linux")]
            if let Err(group_error) = wait_for_process_group_termination(process_group).await {
                return match wait_for_child_exit(child).await {
                    Ok(status) => match crate::process_activity::process_group_has_live_members(
                        process_group,
                    ) {
                        Ok(false) => Ok(status),
                        Ok(true) => Err(group_error.context(format!(
                            "child process was reaped but process group {process_group} cleanup failed"
                        ))),
                        Err(liveness_error) => Err(group_error.context(format!(
                            "child process was reaped but process group {process_group} cleanup failed; final liveness check failed: {liveness_error:#}"
                        ))),
                    },
                    Err(child_error) => Err(group_error.context(format!(
                        "process group {process_group} cleanup failed and direct child reap also failed: {child_error:#}"
                    ))),
                };
            }
            return wait_for_child_exit(child).await;
        }
    }

    #[cfg(not(unix))]
    if child.id().is_some() {
        child.start_kill().context("failed to kill child process")?;
    }
    wait_for_child_exit(child).await
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reaped_child_and_gone_group_ignore_cleanup_timeout() {
        let mut child = Command::new("sh")
            .args(["-c", "exit 0"])
            .process_group(0)
            .spawn()
            .expect("spawn child");
        let process_group = i32::try_from(child.id().expect("child PID")).expect("pid_t range");
        let observed_status = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match inspect_child_without_reaping(&mut child).expect("inspect child") {
                    ChildWaitState::Running => tokio::task::yield_now().await,
                    ChildWaitState::Exited(status) => break status,
                }
            }
        })
        .await
        .expect("child exits before cleanup");
        assert!(
            observed_status.success(),
            "expected the child to exit successfully"
        );
        FORCE_GROUP_CLEANUP_TIMEOUT_FOR.store(process_group, Ordering::SeqCst);

        let status = terminate_child_process_group(&mut child, Duration::ZERO)
            .await
            .expect("a reaped child with no live group members is complete");
        assert!(status.success(), "expected the child to exit successfully");
        assert_eq!(
            FORCE_GROUP_CLEANUP_TIMEOUT_FOR.load(Ordering::SeqCst),
            0,
            "cleanup-timeout injection must be consumed by the cleanup seam",
        );
    }
}
