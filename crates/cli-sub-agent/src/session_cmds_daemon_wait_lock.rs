use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::io::RawFd;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub(crate) struct SessionWaitLock {
    file: std::fs::File,
}

impl SessionWaitLock {
    #[cfg(test)]
    pub(crate) fn raw_fd(&self) -> std::os::unix::io::RawFd {
        self.file.as_raw_fd()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SessionWaitLockDiagnostic {
    pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pid_start_time_ticks: Option<u64>,
}

impl Drop for SessionWaitLock {
    fn drop(&mut self) {
        // SAFETY: `self.file` owns a valid fd for the lock file.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

pub(crate) fn try_acquire_session_wait_lock(session_dir: &Path) -> Result<Option<SessionWaitLock>> {
    let lock_path = session_dir.join(".wait.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;

    let fd = file.as_raw_fd();

    if try_flock_session_wait_file(fd)? {
        return Ok(Some(finalize_wait_lock(file, fd, &lock_path)?));
    }

    // flock failed. If the on-disk diagnostic shows a dead PID, the kernel has
    // already released the flock — retry on the SAME fd without removing the file
    // (avoids TOCTOU: remove_file could unlink a lock acquired between our check
    // and the unlink).
    if is_wait_lock_diagnostic_pid_dead(&lock_path) && try_flock_session_wait_file(fd)? {
        return Ok(Some(finalize_wait_lock(file, fd, &lock_path)?));
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

fn finalize_wait_lock(file: std::fs::File, fd: RawFd, lock_path: &Path) -> Result<SessionWaitLock> {
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

    let mut lock = SessionWaitLock { file };
    write_session_wait_lock_diagnostic(&mut lock.file)?;
    Ok(lock)
}

fn write_session_wait_lock_diagnostic(file: &mut std::fs::File) -> Result<()> {
    let pid = std::process::id();
    let diagnostic = SessionWaitLockDiagnostic {
        pid,
        pid_start_time_ticks: process_start_time_ticks(pid),
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

fn read_wait_lock_diagnostic(lock_path: &Path) -> Option<SessionWaitLockDiagnostic> {
    let mut contents = String::new();
    std::fs::File::open(lock_path)
        .ok()?
        .read_to_string(&mut contents)
        .ok()?;
    serde_json::from_str(&contents).ok()
}

#[cfg(target_os = "linux")]
fn process_start_time_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(") ")?.1;
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn process_start_time_ticks(_pid: u32) -> Option<u64> {
    None
}

fn is_pid_dead(pid: u32, pid_start_time_ticks: Option<u64>) -> bool {
    let Ok(pid_i32) = i32::try_from(pid) else {
        return false;
    };
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
