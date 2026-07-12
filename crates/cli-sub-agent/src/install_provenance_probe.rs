//! Bounded `--version` process-group probe for install/doctor provenance checks.
//!
//! Safety: negative-PGID signals only while this handle owns an unreaped leader.
//! Wait-state unknown (ECHILD / auto-reap) must never signal `-pgid`.

use anyhow::{Context, Result, bail};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Bound for artifact/`csa --version` probes (doctor + install verification).
pub(crate) const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// Cap stdout/stderr growth so a hostile/hung artifact cannot exhaust memory.
pub(crate) const VERSION_PROBE_MAX_BYTES: usize = 64 * 1024;
/// Grace between SIGTERM and SIGKILL while the unreaped leader still anchors the PGID.
const VERSION_PROBE_TERM_GRACE: Duration = Duration::from_millis(100);
/// Bound for post-signal `try_wait` reaping — never use unbounded `Child::wait()`.
const VERSION_PROBE_KILL_WAIT: Duration = Duration::from_secs(2);
const VERSION_PROBE_POLL: Duration = Duration::from_millis(10);

/// Why the wait loop stopped.
///
/// Group cleanup runs afterward **only** while the unreaped leader still anchors
/// the process group (Unix). `OwnershipUnknown` is the exception: wait-state
/// loss means the PID/PGID must not be signaled (same contract as
/// `csa_process` `SpawnedProcessCleanup::WaitStateUnknown`).
#[derive(Debug)]
enum ProbeStopReason {
    LeaderExited,
    TimedOut,
    OutputTooLarge,
    /// waitid/wait failed (e.g. ECHILD after auto-reap); never signal PID/PGID.
    OwnershipUnknown(std::io::Error),
    /// Output size stat failed while leader ownership still holds; still clean.
    OutputStatError(std::io::Error),
}

/// Owns a version-probe child and enforces process-group cleanup ownership rules.
///
/// On Unix, negative-PGID signals are legal only while the group leader remains
/// unreaped. Reaping first can free the numeric PID/PGID for reuse by an
/// unrelated process group — never signal `-pgid` after the leader is reaped.
///
/// If the wait state becomes unknown (for example `waitid` returns `ECHILD`
/// because `SIGCHLD` auto-reaping or another reaper consumed the status),
/// `ownership_unknown` is set and **no** PID or negative-PGID signals may be
/// emitted.
struct VersionProbeSession {
    /// Present until the leader has been reaped (or Drop fails closed).
    child: Option<Child>,
    /// Cached leader status when reaped early (non-Unix `try_wait` path).
    reaped_status: Option<ExitStatus>,
    /// Once true, cleanup must not signal PID/PGID (identity ownership lost).
    ownership_unknown: bool,
}

impl VersionProbeSession {
    fn new(child: Child) -> Self {
        Self {
            child: Some(child),
            reaped_status: None,
            ownership_unknown: false,
        }
    }

    fn child_mut(&mut self) -> Option<&mut Child> {
        self.child.as_mut()
    }

    fn mark_ownership_unknown(&mut self) {
        self.ownership_unknown = true;
    }

    /// Drop the child handle without any PID/PGID signal.
    ///
    /// Exact-child non-blocking `try_wait` is allowed. Never issue `kill` /
    /// `kill(-pgid)` after ownership is unknown.
    fn abandon_without_signaling(&mut self) {
        self.ownership_unknown = true;
        if let Some(mut child) = self.child.take() {
            let _ = child.try_wait();
        }
    }

    /// Signal remaining probe processes while the leader is still unreaped.
    ///
    /// - When the leader already exited (Unix WNOWAIT path): SIGKILL the group
    ///   immediately to reap descendants that may still hold stdout/stderr FDs.
    /// - When the leader is still running (timeout / oversized output): SIGTERM,
    ///   grace window (leader still unreaped), then SIGKILL.
    /// - No-op if the leader was already reaped (non-Unix `try_wait` path).
    /// - No-op if wait-state ownership is unknown (must not target a reused PGID).
    fn terminate_group_while_owned(&mut self, leader_already_exited: bool) {
        if self.ownership_unknown {
            return;
        }
        let Some(child) = self.child.as_mut() else {
            // Already reaped — never signal a negative PGID after ownership loss.
            return;
        };
        let pid = child.id();
        if pid <= 1 {
            return;
        }

        #[cfg(unix)]
        {
            let pgid = -(pid as libc::pid_t);
            if !leader_already_exited {
                // SAFETY: leader is still unreaped, so its PID remains the process-group
                // anchor created via `process_group(0)`. Negative PGID targets that group.
                let _ = unsafe { libc::kill(pgid, libc::SIGTERM) };
                std::thread::sleep(VERSION_PROBE_TERM_GRACE);
            }
            // SAFETY: leader remains unreaped, so its PID still anchors the process group
            // created via `process_group(0)`. Final SIGKILL targets that group only
            // (never after reaping — no post-reap -pgid).
            let kill_rc = unsafe { libc::kill(pgid, libc::SIGKILL) };
            if kill_rc != 0 {
                // ESRCH: group already empty. Other errors: fall back to exact child kill.
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::ESRCH) {
                    let _ = child.kill();
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = leader_already_exited;
            let _ = child.kill();
        }
    }

    fn reap_leader(&mut self) -> Result<ExitStatus> {
        if let Some(status) = self.reaped_status {
            return Ok(status);
        }
        if self.ownership_unknown {
            bail!(
                "cannot safely signal or reap version probe after its child wait state became unknown"
            );
        }
        let mut child = self
            .child
            .take()
            .context("version probe leader already reaped")?;
        // Deadline-limited try_wait only: unbounded Child::wait() would hang the
        // install/doctor path when SIGKILL fails (uninterruptible I/O, etc.).
        let status = match wait_and_reap_for_exit(&mut child, VERSION_PROBE_KILL_WAIT)? {
            Some(status) => status,
            None => {
                // Abandon the Child handle (std::process::Child Drop does not wait)
                // rather than block forever; the process may remain a zombie until
                // this parent exits, which is preferable to hanging just install.
                bail!(
                    "timed out after {} ms reaping version probe leader after terminate",
                    VERSION_PROBE_KILL_WAIT.as_millis()
                );
            }
        };
        self.reaped_status = Some(status);
        Ok(status)
    }

    /// Terminate remaining group members (while owned), then reap the leader.
    ///
    /// `OwnershipUnknown` abandons without any PID/PGID signal.
    fn finish(mut self, stop: &ProbeStopReason) -> Result<ExitStatus> {
        if self.ownership_unknown || matches!(stop, ProbeStopReason::OwnershipUnknown(_)) {
            self.abandon_without_signaling();
            bail!(
                "cannot safely signal or reap version probe after its child wait state became unknown"
            );
        }
        let leader_already_exited = matches!(stop, ProbeStopReason::LeaderExited);
        self.terminate_group_while_owned(leader_already_exited);
        self.reap_leader()
    }
}

impl Drop for VersionProbeSession {
    fn drop(&mut self) {
        if self.child.is_none() {
            return;
        }
        if self.ownership_unknown {
            // Identity ownership lost: exact try_wait only, never -pgid signals.
            self.abandon_without_signaling();
            return;
        }
        // Fail-closed cleanup: escalate as if the probe is still live.
        // Reap is also deadline-bounded so Drop cannot hang the process forever.
        self.terminate_group_while_owned(false);
        if let Some(mut child) = self.child.take() {
            let _ = wait_and_reap_for_exit(&mut child, VERSION_PROBE_KILL_WAIT);
            // If still unreaped after the bound, drop the handle without waiting.
        }
    }
}

/// Poll `try_wait` until the child exits or `timeout` elapses.
///
/// Returns `Ok(None)` on timeout so callers can fail closed without blocking.
fn wait_and_reap_for_exit(child: &mut Child, timeout: Duration) -> Result<Option<ExitStatus>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .context("failed to inspect version probe leader")?
        {
            return Ok(Some(status));
        }
        if started.elapsed() >= timeout {
            return Ok(None);
        }
        std::thread::sleep(VERSION_PROBE_POLL.min(timeout.saturating_sub(started.elapsed())));
    }
}

/// Bounded `--version` probe: timeout, dual-stream size cap, process-group lifecycle.
///
/// Trusted paths only (callers enforce). Never mutates PATH entries.
///
/// Lifecycle:
/// 1. Spawn in a new process group (Unix) so descendants share the leader PGID.
/// 2. Poll until leader exit / timeout / stdout|stderr size breach — on Unix,
///    detect exit with `waitid(WNOWAIT)` so the leader stays unreaped.
/// 3. SIGTERM → grace → SIGKILL the process group while the leader is still owned.
/// 4. Reap the leader exactly once with a deadline-limited `try_wait` loop
///    (`VERSION_PROBE_KILL_WAIT`); never unbounded `Child::wait()`, and never
///    signal a negative PGID after reaping.
/// 5. Enforce both stream caps on the captured output (tempfile-backed; no reader threads).
pub(crate) fn version_output_with_limits(
    path: &Path,
    timeout: Duration,
    max_bytes: usize,
) -> Result<String> {
    let mut stdout_file =
        tempfile::tempfile().context("failed to allocate version probe stdout buffer")?;
    let stderr_file =
        tempfile::tempfile().context("failed to allocate version probe stderr buffer")?;

    let mut cmd = Command::new(path);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            stdout_file
                .try_clone()
                .context("failed to clone version probe stdout")?,
        ))
        .stderr(Stdio::from(
            stderr_file
                .try_clone()
                .context("failed to clone version probe stderr")?,
        ));

    // Isolate process group so cleanup can terminate the probe and grandchildren.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = cmd
        .spawn()
        .with_context(|| format!("failed to run {} --version", path.display()))?;
    let mut session = VersionProbeSession::new(child);

    let stop = poll_version_probe(&mut session, &stdout_file, &stderr_file, timeout, max_bytes);

    // Ownership-unknown: the unreaped-leader anchor may already be gone (ECHILD /
    // auto-reap). Never finish() with group signals — abandon the handle only.
    if let ProbeStopReason::OwnershipUnknown(error) = stop {
        session.abandon_without_signaling();
        return Err(error)
            .with_context(|| format!("failed waiting for {} --version", path.display()));
    }

    // Clean the group while ownership holds, including the success path where the
    // leader already exited but descendants may retain stdout/stderr FDs. Also
    // covers OutputStatError: stat failed but the leader is still ours.
    let status = session.finish(&stop).with_context(|| {
        format!(
            "failed to terminate/reap version probe for {}",
            path.display()
        )
    })?;

    match stop {
        ProbeStopReason::TimedOut => {
            bail!(
                "{} --version timed out after {}ms",
                path.display(),
                timeout.as_millis()
            );
        }
        ProbeStopReason::OutputTooLarge => {
            bail!(
                "{} --version produced more than {} bytes of output",
                path.display(),
                max_bytes
            );
        }
        ProbeStopReason::OutputStatError(error) => {
            return Err(error).with_context(|| {
                format!("failed to inspect {} --version output size", path.display())
            });
        }
        ProbeStopReason::OwnershipUnknown(_) => {
            // Handled above before finish().
            unreachable!("OwnershipUnknown returns before finish");
        }
        ProbeStopReason::LeaderExited => {}
    }

    if !status.success() {
        bail!("{} --version exited with {}", path.display(), status);
    }

    // Post-completion dual-stream cap: descendants may have written after the
    // leader closed its own FDs but before group cleanup completed.
    if file_len_exceeds(&stdout_file, max_bytes)? || file_len_exceeds(&stderr_file, max_bytes)? {
        bail!(
            "{} --version produced more than {} bytes of output",
            path.display(),
            max_bytes
        );
    }

    let mut stdout_bytes = Vec::new();
    stdout_file
        .seek(SeekFrom::Start(0))
        .context("failed to rewind version probe stdout")?;
    stdout_file
        .read_to_end(&mut stdout_bytes)
        .context("failed to read version probe stdout")?;
    if stdout_bytes.len() > max_bytes {
        bail!(
            "{} --version produced more than {} bytes of output",
            path.display(),
            max_bytes
        );
    }

    String::from_utf8(stdout_bytes)
        .map(|value| value.trim().to_string())
        .context("csa --version returned non-UTF-8 output")
}

fn poll_version_probe(
    session: &mut VersionProbeSession,
    stdout_file: &std::fs::File,
    stderr_file: &std::fs::File,
    timeout: Duration,
    max_bytes: usize,
) -> ProbeStopReason {
    let deadline = Instant::now() + timeout;
    loop {
        match probe_leader_state(session) {
            Ok(LeaderPoll::Exited) => return ProbeStopReason::LeaderExited,
            Ok(LeaderPoll::Running) => {}
            Err(error) => return ProbeStopReason::OwnershipUnknown(error),
        }

        match (
            file_len_exceeds(stdout_file, max_bytes),
            file_len_exceeds(stderr_file, max_bytes),
        ) {
            (Ok(true), _) | (_, Ok(true)) => return ProbeStopReason::OutputTooLarge,
            (Err(error), _) | (_, Err(error)) => {
                // Stat failures still own the unreaped leader — fail closed after
                // controlled group cleanup (do not treat as ownership-unknown).
                return ProbeStopReason::OutputStatError(std::io::Error::other(error.to_string()));
            }
            (Ok(false), Ok(false)) => {}
        }

        if Instant::now() >= deadline {
            return ProbeStopReason::TimedOut;
        }
        std::thread::sleep(
            VERSION_PROBE_POLL.min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

enum LeaderPoll {
    Running,
    Exited,
}

/// Inspect leader liveness without releasing process-group ownership on Unix.
fn probe_leader_state(session: &mut VersionProbeSession) -> std::io::Result<LeaderPoll> {
    #[cfg(unix)]
    {
        let Some(child) = session.child_mut() else {
            // Already reaped — treat as exited (should not happen on Unix poll path).
            return Ok(LeaderPoll::Exited);
        };
        // WNOWAIT leaves the exited leader waitable so its PID remains the PGID anchor.
        match leader_exited_without_reaping(child.id()) {
            Ok(exited) => Ok(if exited {
                LeaderPoll::Exited
            } else {
                LeaderPoll::Running
            }),
            Err(error) => {
                // ECHILD / other waitid failures mean ownership may already be gone.
                // Mark before returning so Drop cannot emit a negative-PGID signal.
                session.mark_ownership_unknown();
                Err(error)
            }
        }
    }
    #[cfg(not(unix))]
    {
        // No process-group ownership model: try_wait reaps the direct child.
        let wait_result = match session.child.as_mut() {
            None => return Ok(LeaderPoll::Exited),
            Some(child) => match child.try_wait() {
                Ok(result) => result,
                Err(error) => {
                    session.mark_ownership_unknown();
                    return Err(error);
                }
            },
        };
        match wait_result {
            Some(status) => {
                // Consume the handle after try_wait reaped the child.
                session.child = None;
                session.reaped_status = Some(status);
                Ok(LeaderPoll::Exited)
            }
            None => Ok(LeaderPoll::Running),
        }
    }
}

#[cfg(unix)]
fn leader_exited_without_reaping(pid: u32) -> std::io::Result<bool> {
    #[cfg(test)]
    if let Some(forced) = test_hooks::take_forced_waitid_result() {
        return forced;
    }
    if pid <= 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid version probe PID {pid}"),
        ));
    }
    loop {
        // SAFETY: zeroed siginfo is the documented no-state-change baseline for waitid.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: info points to writable storage; WNOWAIT keeps the child unreaped.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if rc == -1 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        // SAFETY: waitid initialized info on success; si_pid == 0 means no change.
        let exited_pid = unsafe { info.si_pid() };
        return Ok(exited_pid != 0);
    }
}

/// Test-only hooks for deterministic ownership/wait regressions.
#[cfg(all(test, unix))]
mod test_hooks {
    use std::cell::RefCell;
    use std::sync::Mutex;

    thread_local! {
        /// When set, the next `leader_exited_without_reaping` call returns this
        /// result once (then clears). Used to inject ECHILD / wait-state loss.
        static FORCED_WAITID: RefCell<Option<std::io::Result<bool>>> = const { RefCell::new(None) };
    }

    /// Serialize tests that mutate process-group fixtures + hooks.
    pub(super) static HOOK_LOCK: Mutex<()> = Mutex::new(());

    pub(super) fn force_waitid_once(result: std::io::Result<bool>) {
        FORCED_WAITID.with(|slot| {
            *slot.borrow_mut() = Some(result);
        });
    }

    pub(super) fn take_forced_waitid_result() -> Option<std::io::Result<bool>> {
        FORCED_WAITID.with(|slot| slot.borrow_mut().take())
    }

    pub(super) fn clear() {
        FORCED_WAITID.with(|slot| {
            *slot.borrow_mut() = None;
        });
    }
}

fn file_len_exceeds(file: &std::fs::File, max_bytes: usize) -> Result<bool> {
    let len = file
        .metadata()
        .context("failed to stat version probe output")?
        .len();
    Ok(len as usize > max_bytes)
}

#[cfg(all(test, unix))]
#[path = "install_provenance_probe_tests.rs"]
mod install_provenance_probe_tests;
