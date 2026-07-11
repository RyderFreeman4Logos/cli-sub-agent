use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::fd::{FromRawFd, OwnedFd};
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::RawFd;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Caller-owned result channel used as the wait invocation's lifecycle lease.
///
/// The output descriptor is inherited from the launcher before this process
/// can run, so it remains authoritative even if the waiter has already been
/// reparented. A closed pipe/socket reports HUP or ERR through `poll(2)`;
/// regular files and terminals remain valid result sinks without relying on a
/// sampled parent PID.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct WaitCallerIdentity {
    output_fd: Option<RawFd>,
    output_device: Option<u64>,
    output_inode: Option<u64>,
}

impl WaitCallerIdentity {
    pub(crate) fn capture() -> Self {
        Self::from_output_fd(libc::STDOUT_FILENO)
    }

    /// Validate that the startup result channel still owns this invocation.
    pub(crate) fn validate_for_wait(self) -> Result<Self> {
        self.verified_parts().ok_or_else(|| {
            anyhow::anyhow!(
                "cannot establish session wait caller lifecycle: stdout identity unavailable"
            )
        })?;
        if self.is_dead() {
            anyhow::bail!(
                "session wait caller output closed before wait initialization; re-issue from the current caller"
            );
        }

        Ok(self)
    }

    fn from_output_fd(output_fd: RawFd) -> Self {
        let Some((output_device, output_inode)) = descriptor_identity(output_fd) else {
            return Self::default();
        };
        Self {
            output_fd: Some(output_fd),
            output_device: Some(output_device),
            output_inode: Some(output_inode),
        }
    }

    fn verified_parts(self) -> Option<(RawFd, u64, u64)> {
        let (output_fd, output_device, output_inode) = self
            .output_fd
            .zip(self.output_device)
            .zip(self.output_inode)
            .map(|((fd, device), inode)| (fd, device, inode))?;
        (descriptor_identity(output_fd) == Some((output_device, output_inode))).then_some((
            output_fd,
            output_device,
            output_inode,
        ))
    }

    pub(crate) fn is_dead(self) -> bool {
        let Some((output_fd, _, _)) = self.verified_parts() else {
            return self.output_fd.is_some();
        };
        let mut poll_fd = libc::pollfd {
            fd: output_fd,
            events: 0,
            revents: 0,
        };
        // SAFETY: `poll_fd` points to one initialized descriptor entry and a
        // zero timeout makes this a non-blocking lifecycle probe.
        let poll_result = unsafe { libc::poll(&mut poll_fd, 1, 0) };
        poll_result > 0 && poll_fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
    }

    #[cfg(test)]
    pub(crate) fn from_output_fd_for_test(output_fd: RawFd) -> Self {
        Self::from_output_fd(output_fd)
    }

    #[cfg(test)]
    pub(crate) fn diagnostic_parts_for_test(self) -> Option<(u64, u64)> {
        self.verified_parts()
            .map(|(_, output_device, output_inode)| (output_device, output_inode))
    }
}

fn descriptor_identity(fd: RawFd) -> Option<(u64, u64)> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `stat` points to writable storage for one `libc::stat`; `fstat`
    // initializes it on success and does not retain the pointer.
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: the successful `fstat` call initialized the entire structure.
    let stat = unsafe { stat.assume_init() };
    #[cfg(target_os = "linux")]
    let identity = (stat.st_dev, stat.st_ino);
    #[cfg(not(target_os = "linux"))]
    let identity = (stat.st_dev as u64, stat.st_ino as u64);
    Some(identity)
}

pub(crate) struct SessionWaitLock {
    file: std::fs::File,
    caller_identity: WaitCallerIdentity,
}

impl SessionWaitLock {
    #[cfg(test)]
    pub(crate) fn raw_fd(&self) -> std::os::unix::io::RawFd {
        self.file.as_raw_fd()
    }

    pub(crate) fn caller_identity(&self) -> WaitCallerIdentity {
        self.caller_identity
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SessionWaitLockDiagnostic {
    pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pid_start_time_ticks: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    caller_output_device: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    caller_output_inode: Option<u64>,
    /// Legacy compatibility for diagnostics written before the caller-owned
    /// output lifecycle contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent_pid: Option<u32>,
}

impl Drop for SessionWaitLock {
    fn drop(&mut self) {
        // SAFETY: `self.file` owns a valid fd for the lock file.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[cfg(test)]
pub(crate) fn try_acquire_session_wait_lock(session_dir: &Path) -> Result<Option<SessionWaitLock>> {
    try_acquire_session_wait_lock_with_caller(session_dir, WaitCallerIdentity::capture())
}

pub(crate) fn try_acquire_session_wait_lock_with_caller(
    session_dir: &Path,
    caller_identity: WaitCallerIdentity,
) -> Result<Option<SessionWaitLock>> {
    let lock_path = session_dir.join(".wait.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;

    let fd = file.as_raw_fd();

    if try_flock_session_wait_file(fd)? {
        return Ok(Some(finalize_wait_lock(
            file,
            fd,
            &lock_path,
            caller_identity,
        )?));
    }

    // flock failed. If the on-disk diagnostic shows a dead PID, the kernel has
    // already released the flock — retry on the SAME fd without removing the file
    // (avoids TOCTOU: remove_file could unlink a lock acquired between our check
    // and the unlink).
    if is_wait_lock_diagnostic_pid_dead(&lock_path) && try_flock_session_wait_file(fd)? {
        return Ok(Some(finalize_wait_lock(
            file,
            fd,
            &lock_path,
            caller_identity,
        )?));
    }

    // Older diagnostics used a sampled PPID. Preserve their verified reclaim
    // path while current waiters release on caller-output closure themselves.
    if let Some(validated) = is_wait_lock_diagnostic_caller_gone(&lock_path)
        && descriptor_identity(fd)
            .is_some_and(|identity| kill_wait_lock_holder(&lock_path, identity, validated))
    {
        // The verified orphan wait process was signaled. Its exit releases the
        // kernel flock, so retry acquisition for a bounded interval.
        // Bounded retry: poll the flock in a deadline loop instead of a
        // single fixed sleep. Under high load the process may take longer
        // to exit and release the flock.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if try_flock_session_wait_file(fd)? {
                return Ok(Some(finalize_wait_lock(
                    file,
                    fd,
                    &lock_path,
                    caller_identity,
                )?));
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    // Drop file to close the fd we opened (flock not acquired).
    drop(file);
    Ok(None)
}

fn try_flock_session_wait_file(fd: RawFd) -> Result<bool> {
    // SAFETY: fd is valid and LOCK_EX | LOCK_NB is a standard non-blocking
    // advisory exclusive lock request.
    let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(true);
    }

    let err = std::io::Error::last_os_error();
    if matches!(
        err.raw_os_error(),
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
    ) {
        return Ok(false);
    }

    Err(err.into())
}

fn finalize_wait_lock(
    file: std::fs::File,
    fd: RawFd,
    lock_path: &Path,
    caller_identity: WaitCallerIdentity,
) -> Result<SessionWaitLock> {
    let pid = std::process::id();

    // Set close-on-exec so spawned subprocesses don't inherit the wait lock.
    // SAFETY: fd is valid and F_GETFD only reads descriptor flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 {
        return Err(anyhow::anyhow!(
            "F_GETFD failed for wait lock at {}: {}",
            lock_path.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: fd is valid, F_SETFD sets close-on-exec.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == -1 {
        return Err(anyhow::anyhow!(
            "F_SETFD failed for wait lock at {}: {}",
            lock_path.display(),
            std::io::Error::last_os_error()
        ));
    }

    let mut lock_file = file;
    write_session_wait_lock_diagnostic(&mut lock_file, pid, caller_identity)?;
    Ok(SessionWaitLock {
        file: lock_file,
        caller_identity,
    })
}

fn write_session_wait_lock_diagnostic(
    file: &mut std::fs::File,
    pid: u32,
    caller_identity: WaitCallerIdentity,
) -> Result<()> {
    let caller_parts = caller_identity.verified_parts();
    let diagnostic = SessionWaitLockDiagnostic {
        pid,
        pid_start_time_ticks: process_start_time_ticks(pid),
        caller_output_device: caller_parts.map(|(_, device, _)| device),
        caller_output_inode: caller_parts.map(|(_, _, inode)| inode),
        parent_pid: None,
    };
    let json = serde_json::to_string(&diagnostic)?;

    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(json.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

fn is_wait_lock_diagnostic_pid_dead(lock_path: &Path) -> bool {
    let Some(diagnostic) = read_wait_lock_diagnostic(lock_path) else {
        return false;
    };
    if !is_pid_dead(diagnostic.pid, diagnostic.pid_start_time_ticks) {
        return false;
    }

    tracing::warn!(
        lock_path = %lock_path.display(),
        holder_pid = diagnostic.pid,
        "retrying session wait lock flock after detecting dead holder PID"
    );
    true
}

/// Return a verified live legacy holder whose recorded parent changed.
///
/// Current waiters observe their caller-owned output channel directly and
/// release the flock themselves. This PPID comparison is retained only so a
/// new binary can safely reclaim locks written by older versions.
fn is_wait_lock_diagnostic_caller_gone(lock_path: &Path) -> Option<SessionWaitLockDiagnostic> {
    let diagnostic = read_wait_lock_diagnostic(lock_path)?;
    let pid_start_time_ticks = diagnostic.pid_start_time_ticks.filter(|ticks| *ticks != 0);

    // Require a non-zero start-time for holder identity verification. Without
    // it, a recycled PID could cause us to SIGTERM an unrelated process.
    let expected_start_time = pid_start_time_ticks?;

    // Verify the PID's start-time matches the recorded value (catches recycle).
    match process_start_time_ticks(diagnostic.pid) {
        Some(actual) if actual != expected_start_time => return None, // PID recycled.
        None => return None, // Can't verify identity — don't kill.
        _ => {}
    }

    // If the PID is dead, that path is already handled by is_wait_lock_diagnostic_pid_dead.
    if is_pid_dead(diagnostic.pid, Some(expected_start_time)) {
        return None;
    }

    let recorded_ppid = diagnostic.parent_pid?;
    let current_ppid = parent_pid(diagnostic.pid)?;

    if current_ppid == recorded_ppid {
        return None; // Same parent — not orphaned.
    }

    tracing::warn!(
        lock_path = %lock_path.display(),
        holder_pid = diagnostic.pid,
        recorded_ppid,
        current_ppid,
        "retrying legacy session wait lock flock after detecting reparented holder PID"
    );
    Some(diagnostic)
}

fn read_wait_lock_diagnostic(lock_path: &Path) -> Option<SessionWaitLockDiagnostic> {
    let mut contents = String::new();
    std::fs::File::open(lock_path)
        .ok()?
        .read_to_string(&mut contents)
        .ok()?;
    serde_json::from_str(&contents).ok()
}

#[cfg(target_os = "linux")]
pub(crate) fn process_start_time_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(") ")?.1;
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_os = "linux")]
pub(crate) fn process_state(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(") ")?.1;
    after_comm.chars().next()
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn process_state(_pid: u32) -> Option<char> {
    None
}

/// macOS: PID/start-time based legacy reclaim is not supported. Current
/// waiters still release their lock when the caller-owned output closes.
#[cfg(target_os = "macos")]
pub(crate) fn process_start_time_ticks(_pid: u32) -> Option<u64> {
    None
}

/// Fallback for other platforms (e.g. BSD, Windows): no process start time.
/// Orphan reclaim is Linux-only.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn process_start_time_ticks(_pid: u32) -> Option<u64> {
    None
}

/// Read the parent PID (ppid) from /proc/<pid>/stat.
/// Linux /proc stat fields (after comm): state(0), ppid(1), ... start_time(19).
#[cfg(target_os = "linux")]
pub(crate) fn parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(") ")?.1;
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

/// macOS: parent_pid is not available (orphan reclaim is Linux-only).
#[cfg(target_os = "macos")]
pub(crate) fn parent_pid(_pid: u32) -> Option<u32> {
    None
}

/// Fallback for other platforms: no parent PID reading.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn parent_pid(_pid: u32) -> Option<u32> {
    None
}

/// Send SIGTERM to a verified wait lock holder to release the kernel flock.
///
/// The diagnostic file is advisory metadata and may be rewritten while another
/// process holds the flock. Before signaling, bind the target process to the
/// contested lock's device/inode through `/proc/<pid>/fdinfo`, then use a pidfd
/// so PID reuse between verification and signal delivery cannot target a new
/// process.
#[cfg(target_os = "linux")]
fn kill_wait_lock_holder(
    lock_path: &Path,
    lock_identity: (u64, u64),
    validated: SessionWaitLockDiagnostic,
) -> bool {
    // Zero is an explicit sentinel for diagnostics written without a usable
    // start-time identity. Refuse to signal without start-time verification
    // (rule 018: never act on PID-only identity).
    let expected_start_time = validated.pid_start_time_ticks.filter(|ticks| *ticks != 0);

    // Fresh liveness check, including start-time identity when available.
    if is_pid_dead(validated.pid, expected_start_time) {
        return false; // Already dead — flock will be released by the kernel.
    }

    let Some(pidfd) = open_pidfd(validated.pid) else {
        return false;
    };

    // Re-check start-time after opening the pidfd. The pidfd pins signal
    // delivery to this process even if the numeric PID is later recycled.
    let Some(expected_start_time) = expected_start_time else {
        return false;
    };
    match process_start_time_ticks(validated.pid) {
        Some(actual) if actual != expected_start_time => {
            tracing::warn!(
                lock_path = %lock_path.display(),
                holder_pid = validated.pid,
                "refusing to SIGTERM wait lock holder: PID start-time mismatch (recycled)"
            );
            return false;
        }
        None => return false,
        _ => {}
    }

    let Some(recorded_ppid) = validated.parent_pid else {
        return false;
    };
    match parent_pid(validated.pid) {
        None => return false,
        Some(current_ppid) if current_ppid == recorded_ppid => {
            return false;
        }
        _ => {}
    }

    if !process_holds_exclusive_flock(validated.pid, lock_identity) {
        tracing::warn!(
            lock_path = %lock_path.display(),
            diagnostic_pid = validated.pid,
            "refusing to SIGTERM wait lock diagnostic PID: process does not hold the contested flock"
        );
        return false;
    }

    if let Err(error) = pidfd_send_sigterm(&pidfd) {
        tracing::warn!(
            lock_path = %lock_path.display(),
            holder_pid = validated.pid,
            %error,
            "failed to SIGTERM orphaned wait lock holder"
        );
        false
    } else {
        tracing::info!(
            lock_path = %lock_path.display(),
            holder_pid = validated.pid,
            "sent SIGTERM to orphaned wait lock holder"
        );
        true
    }
}

#[cfg(not(target_os = "linux"))]
fn kill_wait_lock_holder(
    _lock_path: &Path,
    _lock_identity: (u64, u64),
    _validated: SessionWaitLockDiagnostic,
) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn process_holds_exclusive_flock(pid: u32, lock_identity: (u64, u64)) -> bool {
    let Ok(fdinfo_entries) = std::fs::read_dir(format!("/proc/{pid}/fdinfo")) else {
        return false;
    };

    for entry in fdinfo_entries.flatten() {
        let fd_name = entry.file_name();
        let fd_path = Path::new("/proc")
            .join(pid.to_string())
            .join("fd")
            .join(&fd_name);
        let Ok(metadata) = std::fs::metadata(fd_path) else {
            continue;
        };
        if (metadata.dev(), metadata.ino()) != lock_identity {
            continue;
        }

        let Ok(fdinfo) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if fdinfo.lines().any(fdinfo_line_is_exclusive_flock) {
            return true;
        }
    }

    false
}

#[cfg(target_os = "linux")]
fn fdinfo_line_is_exclusive_flock(line: &str) -> bool {
    let Some(lock) = line.strip_prefix("lock:") else {
        return false;
    };
    let mut fields = lock.split_whitespace();
    let _lock_id = fields.next();
    matches!(
        (fields.next(), fields.next(), fields.next()),
        (Some("FLOCK"), Some("ADVISORY"), Some("WRITE"))
    )
}

#[cfg(target_os = "linux")]
fn open_pidfd(pid: u32) -> Option<OwnedFd> {
    let pid = i32::try_from(pid).ok()?;
    // SAFETY: pidfd_open receives a positive PID and zero flags. On success it
    // returns a new owned file descriptor; no pointers cross the FFI boundary.
    let raw_fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if raw_fd < 0 {
        return None;
    }
    let raw_fd = i32::try_from(raw_fd).ok()?;
    // SAFETY: a successful pidfd_open returned a fresh descriptor that is
    // adopted exactly once by OwnedFd.
    Some(unsafe { OwnedFd::from_raw_fd(raw_fd) })
}

#[cfg(target_os = "linux")]
fn pidfd_send_sigterm(pidfd: &OwnedFd) -> std::io::Result<()> {
    // SAFETY: pidfd is live and owned for this call; a null siginfo pointer and
    // zero flags are the documented pidfd_send_signal contract for SIGTERM.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            libc::SIGTERM,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn is_pid_dead(pid: u32, pid_start_time_ticks: Option<u64>) -> bool {
    // Advisory diagnostics do not confer child-process ownership, so this
    // liveness probe must never call waitpid(2) or consume an exit status.
    let Ok(pid_i32) = i32::try_from(pid) else {
        return false;
    };
    if pid_i32 == 0 {
        return false;
    }
    if process_state(pid) == Some('Z') {
        return true;
    }

    // SAFETY: kill(pid, 0) sends no signal; only checks process existence.
    let alive = unsafe { libc::kill(pid_i32, 0) } == 0;
    if !alive {
        let error = std::io::Error::last_os_error();
        return error.raw_os_error() == Some(libc::ESRCH);
    }

    if let Some(expected_ticks) = pid_start_time_ticks
        && let Some(current_ticks) = process_start_time_ticks(pid)
    {
        return current_ticks != expected_ticks;
    }

    false
}
