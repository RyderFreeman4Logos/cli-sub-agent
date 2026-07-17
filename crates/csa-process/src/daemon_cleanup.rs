use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

#[cfg(test)]
use std::cell::RefCell;

use anyhow::{Context, Result};

use super::DaemonSpawnMode;

const SCOPE_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const PROCESS_TERM_GRACE: Duration = Duration::from_millis(100);
const PROCESS_KILL_WAIT: Duration = Duration::from_secs(2);
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(test)]
type BeforeFinalGroupSignalObserver = Box<dyn FnOnce(u32)>;

#[cfg(test)]
thread_local! {
    static BEFORE_FINAL_GROUP_SIGNAL_OBSERVER: RefCell<Option<BeforeFinalGroupSignalObserver>> =
        const { RefCell::new(None) };
}

/// Removes a test-only cleanup observer when its fixture scope ends.
#[cfg(test)]
pub(super) struct BeforeFinalGroupSignalObserverGuard;

#[cfg(test)]
impl Drop for BeforeFinalGroupSignalObserverGuard {
    fn drop(&mut self) {
        BEFORE_FINAL_GROUP_SIGNAL_OBSERVER.with(|slot| {
            slot.borrow_mut().take();
        });
    }
}

/// Installs a one-shot observer immediately before the final group signal.
///
/// The hook is thread-local so concurrent tests cannot observe or replace one
/// another's cleanup fixture. Its return value cannot alter cleanup control
/// flow; tests record evidence and assert it only after cleanup completes.
#[cfg(test)]
pub(super) fn observe_before_final_group_signal_for_test(
    observer: impl FnOnce(u32) + 'static,
) -> BeforeFinalGroupSignalObserverGuard {
    BEFORE_FINAL_GROUP_SIGNAL_OBSERVER.with(|slot| {
        let mut slot = slot.borrow_mut();
        assert!(
            slot.is_none(),
            "final group signal observer already installed"
        );
        *slot = Some(Box::new(observer));
    });
    BeforeFinalGroupSignalObserverGuard
}

#[cfg(test)]
fn notify_before_final_group_signal_for_test(pid: u32) {
    let observer = BEFORE_FINAL_GROUP_SIGNAL_OBSERVER.with(|slot| slot.borrow_mut().take());
    if let Some(observer) = observer {
        observer(pid);
    }
}

/// Describes whether cleanup still owns an unreaped process-group leader.
///
/// Negative-PGID signals are only legal for `ProcessGroupAnchor`: reaping the
/// leader releases its PID and can let an unrelated process group reuse the
/// numeric PGID. On non-Unix platforms `AlreadyReaped` and `WaitStateUnknown`
/// therefore permit only exact unit cleanup: a wait error may itself mean child
/// ownership was consumed outside this handle.
pub(super) enum SpawnedProcessCleanup<'a> {
    ProcessGroupAnchor(&'a mut Child),
    #[cfg(not(unix))]
    AlreadyReaped,
    WaitStateUnknown,
}

pub(super) enum SpawnedProcessLiveness {
    Running,
    Exited(String),
    #[cfg(not(unix))]
    AlreadyReaped(ExitStatus),
}

pub(super) fn inspect_spawned_process_without_reaping(
    child: &mut Child,
) -> Result<SpawnedProcessLiveness> {
    #[cfg(unix)]
    {
        inspect_spawned_process_with_waitid(child, waitid_without_reaping)
    }

    #[cfg(not(unix))]
    match child
        .try_wait()
        .context("failed to inspect spawned daemon after readiness verification")?
    {
        Some(status) => Ok(SpawnedProcessLiveness::AlreadyReaped(status)),
        None => Ok(SpawnedProcessLiveness::Running),
    }
}

#[cfg(unix)]
fn inspect_spawned_process_with_waitid(
    child: &mut Child,
    mut waitid: impl FnMut(libc::id_t, &mut libc::siginfo_t) -> std::io::Result<()>,
) -> Result<SpawnedProcessLiveness> {
    let pid = child.id();
    anyhow::ensure!(pid > 1, "invalid spawned daemon PID {pid}");
    loop {
        // SAFETY: zero is the documented no-state-change sentinel for the
        // siginfo_t returned by waitid with WNOHANG.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        match waitid(pid as libc::id_t, &mut info) {
            Ok(()) => {
                // SAFETY: waitid initialized info; si_pid == 0 means no state change.
                let exited_pid = unsafe { info.si_pid() };
                if exited_pid == 0 {
                    return Ok(SpawnedProcessLiveness::Running);
                }
                // SAFETY: this is a SIGCHLD wait result from waitid(P_PID, ...).
                let status = unsafe { info.si_status() };
                return Ok(SpawnedProcessLiveness::Exited(format!(
                    "wait code {} status {status}",
                    info.si_code
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(error).context("failed to inspect spawned daemon with waitid(WNOWAIT)");
            }
        }
    }
}

#[cfg(unix)]
fn waitid_without_reaping(id: libc::id_t, info: &mut libc::siginfo_t) -> std::io::Result<()> {
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

pub(super) fn terminate_and_reap_spawned_daemon(
    process: SpawnedProcessCleanup<'_>,
    spawn_mode: &DaemonSpawnMode,
    systemctl: &Path,
) -> Result<()> {
    let scope_cleanup = match spawn_mode {
        DaemonSpawnMode::Direct => Ok(()),
        DaemonSpawnMode::IndependentScope { unit } => stop_systemd_scope(systemctl, unit),
    };
    let process_cleanup = match process {
        SpawnedProcessCleanup::ProcessGroupAnchor(child) => terminate_and_reap_process_group(child),
        #[cfg(not(unix))]
        SpawnedProcessCleanup::AlreadyReaped => Ok(()),
        SpawnedProcessCleanup::WaitStateUnknown => Err(anyhow::anyhow!(
            "cannot safely signal or reap spawned daemon after its child wait state became unknown"
        )),
    };

    match (scope_cleanup, process_cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(scope_err), Ok(())) => Err(scope_err),
        (Ok(()), Err(process_err)) => Err(process_err),
        (Err(scope_err), Err(process_err)) => Err(scope_err.context(format!(
            "process-group cleanup also failed: {process_err:#}"
        ))),
    }
}

fn stop_systemd_scope(systemctl: &Path, unit: &str) -> Result<()> {
    stop_systemd_scope_with_timeout(systemctl, unit, SCOPE_STOP_TIMEOUT)
}

pub(super) fn stop_systemd_scope_with_timeout(
    systemctl: &Path,
    unit: &str,
    timeout: Duration,
) -> Result<()> {
    let mut command = Command::new(systemctl);
    command
        .args(["--user", "stop", unit])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_new_session(&mut command);

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to run systemctl --user stop {unit}"))?;
    let status = match wait_and_reap_for_exit(&mut child, timeout) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let timeout_err = anyhow::anyhow!(
                "systemctl --user stop {unit} timed out after {} ms",
                timeout.as_millis()
            );
            return match terminate_and_reap_process_group(&mut child) {
                Ok(()) => Err(timeout_err),
                Err(cleanup_err) => Err(timeout_err.context(format!(
                    "timed-out systemctl cleanup also failed: {cleanup_err:#}"
                ))),
            };
        }
        Err(wait_err) => {
            // A wait error can mean another mechanism already consumed the
            // child status (for example ECHILD). Without a proven unreaped
            // leader, neither the PID nor PGID remains a safe signal target.
            return Err(wait_err)
                .context(format!("failed waiting for systemctl --user stop {unit}"));
        }
    };

    anyhow::ensure!(
        status.success(),
        "systemctl --user stop {unit} exited with status {}",
        status
            .code()
            .map_or_else(|| status.to_string(), |code| code.to_string())
    );
    Ok(())
}

fn terminate_and_reap_process_group(child: &mut Child) -> Result<()> {
    let pid = child.id();
    anyhow::ensure!(
        pid > 1,
        "refusing to signal invalid spawned daemon PID {pid}"
    );
    let pgid = -(pid as libc::pid_t);

    // Do not inspect or reap the leader during the TERM grace period. Even if
    // it exits immediately, its unreaped child status keeps the numeric PID
    // unavailable for reuse while we issue the final group signal.
    signal_process_group(pgid, libc::SIGTERM, pid, child)?;
    std::thread::sleep(PROCESS_TERM_GRACE);

    // The owned leader is still unreaped here, so this negative-PGID signal
    // cannot target a process group that reused the leader's numeric PID.
    #[cfg(test)]
    notify_before_final_group_signal_for_test(pid);
    let kill_error = signal_process_group(pgid, libc::SIGKILL, pid, child).err();
    let wait_error = match wait_and_reap_for_exit(child, PROCESS_KILL_WAIT) {
        Ok(Some(_)) => None,
        Ok(None) => Some(anyhow::anyhow!(
            "timed out after {} ms reaping spawned daemon PID {pid} after SIGKILL",
            PROCESS_KILL_WAIT.as_millis()
        )),
        Err(error) => {
            Some(error.context("failed waiting for daemon leader after final group signal"))
        }
    };

    match (kill_error, wait_error) {
        (None, None) => Ok(()),
        (Some(error), None) | (None, Some(error)) => Err(error),
        (Some(kill_error), Some(wait_error)) => {
            Err(kill_error.context(format!("daemon leader wait also failed: {wait_error:#}")))
        }
    }
}

fn signal_process_group(
    pgid: libc::pid_t,
    signal: libc::c_int,
    pid: u32,
    child: &mut Child,
) -> Result<()> {
    // SAFETY: callers derive the negative PGID from the still-owned child.
    // setsid() made that child its group leader before exec.
    let rc = unsafe { libc::kill(pgid, signal) };
    if rc == 0 {
        return Ok(());
    }

    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    if signal == libc::SIGKILL {
        child
            .kill()
            .with_context(|| format!("failed to kill spawned daemon PID {pid}: {error}"))?;
        return Ok(());
    }

    tracing::warn!(pid, %error, "failed to SIGTERM spawned daemon process group");
    Ok(())
}

fn wait_and_reap_for_exit(child: &mut Child, timeout: Duration) -> Result<Option<ExitStatus>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .context("failed to inspect child process")?
        {
            return Ok(Some(status));
        }
        if started.elapsed() >= timeout {
            return Ok(None);
        }
        std::thread::sleep(WAIT_POLL_INTERVAL.min(timeout.saturating_sub(started.elapsed())));
    }
}

fn configure_new_session(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: setsid() is async-signal-safe and is called after fork, before
    // exec, so timeout cleanup can signal only this helper's process group.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_waitid_probe_is_retried() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 5"])
            .spawn()
            .expect("spawn waitid fixture");
        let mut calls = 0;
        let result = inspect_spawned_process_with_waitid(&mut child, |_, _| {
            calls += 1;
            if calls == 1 {
                Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
            } else {
                Ok(())
            }
        });
        let _ = child.kill();
        let _ = child.wait();

        assert!(matches!(result, Ok(SpawnedProcessLiveness::Running)));
        assert_eq!(calls, 2, "waitid must be retried exactly once after EINTR");
    }
}
