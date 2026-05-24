use std::io::{Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub(crate) struct SessionWaitLock {
    file: std::fs::File,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
struct SessionWaitLockDiagnostic {
    pid: u32,
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
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;

    if try_flock_session_wait_file(&file)? {
        write_session_wait_lock_diagnostic(&mut file)?;
        return Ok(Some(SessionWaitLock { file }));
    }

    Ok(None)
}

fn try_flock_session_wait_file(file: &std::fs::File) -> Result<bool> {
    // SAFETY: `file` owns a valid fd and `LOCK_EX | LOCK_NB` is a standard
    // non-blocking advisory exclusive lock request.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
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

fn write_session_wait_lock_diagnostic(file: &mut std::fs::File) -> Result<()> {
    let diagnostic = SessionWaitLockDiagnostic {
        pid: std::process::id(),
    };
    let json = serde_json::to_string(&diagnostic)?;

    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(json.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}
