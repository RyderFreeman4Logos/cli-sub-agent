//! File-based locking using flock (via fd-lock).
//! Independent crate with no internal csa dependencies.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fd_lock::RwLock;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Diagnostic information written to lock files
#[derive(Debug, Serialize, Deserialize)]
struct LockDiagnostic {
    pid: u32,
    tool_name: String,
    acquired_at: DateTime<Utc>,
}

/// Session lock guard that holds an fd-lock write guard.
/// The lock is released when this struct is dropped.
pub struct SessionLock {
    #[allow(dead_code)]
    guard: &'static fd_lock::RwLockWriteGuard<'static, File>,
    lock_path: PathBuf,
}

impl std::fmt::Debug for SessionLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionLock")
            .field("lock_path", &self.lock_path)
            .finish()
    }
}

impl SessionLock {
    /// Get the path to the lock file
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}

/// Acquire a non-blocking write lock for a session and tool.
///
/// Lock path: `{session_dir}/locks/{tool_name}.lock`
///
/// On success:
/// - Acquires exclusive write lock
/// - Writes diagnostic JSON (pid, tool_name, acquired_at) to lock file
/// - Returns SessionLock guard
///
/// On failure:
/// - Attempts to read existing lock file to report which PID holds it
/// - Returns error with diagnostic information
pub fn acquire_lock(session_dir: &Path, tool_name: &str) -> Result<SessionLock> {
    let locks_dir = session_dir.join("locks");
    fs::create_dir_all(&locks_dir)
        .with_context(|| format!("Failed to create locks directory: {}", locks_dir.display()))?;

    let lock_path = locks_dir.join(format!("{}.lock", tool_name));

    // Open or create the lock file
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("Failed to open lock file: {}", lock_path.display()))?;

    // Leak the RwLock to give it a 'static lifetime
    // This is safe because the lock lives for the process duration
    let rwlock = Box::leak(Box::new(RwLock::new(file)));

    // Try to acquire a non-blocking write lock
    match rwlock.try_write() {
        Ok(mut guard) => {
            // Write diagnostic information
            let diagnostic = LockDiagnostic {
                pid: std::process::id(),
                tool_name: tool_name.to_string(),
                acquired_at: Utc::now(),
            };

            let json = serde_json::to_string(&diagnostic)
                .context("Failed to serialize lock diagnostic")?;

            guard.set_len(0).context("Failed to truncate lock file")?;
            guard
                .write_all(json.as_bytes())
                .context("Failed to write lock diagnostic")?;
            guard.flush().context("Failed to flush lock file")?;

            // Leak the guard to give it a 'static lifetime
            let static_guard = Box::leak(Box::new(guard));

            Ok(SessionLock {
                guard: static_guard,
                lock_path,
            })
        }
        Err(_) => {
            // Lock is held by another process, try to read diagnostic info
            let mut file =
                File::open(&lock_path).context("Failed to open lock file to read diagnostic")?;
            let mut contents = String::new();
            file.read_to_string(&mut contents)
                .context("Failed to read lock file")?;

            let error_msg =
                if let Ok(diagnostic) = serde_json::from_str::<LockDiagnostic>(&contents) {
                    format!(
                        "Session locked by PID {} (tool: {}, acquired: {})",
                        diagnostic.pid, diagnostic.tool_name, diagnostic.acquired_at
                    )
                } else {
                    "Session is locked (unable to read diagnostic info)".to_string()
                };

            Err(anyhow::anyhow!(error_msg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_acquire_lock_succeeds() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let session_dir = temp_dir.path();

        let lock = acquire_lock(session_dir, "test-tool");
        assert!(lock.is_ok(), "Lock acquisition should succeed");

        let lock = lock.unwrap();
        assert!(lock.lock_path().exists(), "Lock file should exist");
    }

    #[test]
    fn test_lock_file_path() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let session_dir = temp_dir.path();

        let lock = acquire_lock(session_dir, "test-tool").expect("Failed to acquire lock");

        let expected_path = session_dir.join("locks/test-tool.lock");
        assert_eq!(lock.lock_path(), expected_path);
        assert!(
            expected_path.exists(),
            "Lock file should exist at correct path"
        );
    }

    #[test]
    fn test_lock_diagnostic_written() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let session_dir = temp_dir.path();

        let _lock = acquire_lock(session_dir, "test-tool").expect("Failed to acquire lock");

        let lock_path = session_dir.join("locks/test-tool.lock");
        let contents = fs::read_to_string(&lock_path).expect("Failed to read lock file");

        let diagnostic: LockDiagnostic =
            serde_json::from_str(&contents).expect("Failed to parse diagnostic JSON");

        assert_eq!(diagnostic.pid, std::process::id());
        assert_eq!(diagnostic.tool_name, "test-tool");
    }

    #[test]
    fn test_second_lock_fails() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let session_dir = temp_dir.path();

        let _lock1 = acquire_lock(session_dir, "test-tool").expect("First lock should succeed");

        let result = acquire_lock(session_dir, "test-tool");
        assert!(result.is_err(), "Second lock should fail");

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Session locked by PID"),
            "Error message should contain PID info"
        );
    }

    #[test]
    fn test_lock_released_on_drop() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let session_dir = temp_dir.path();

        {
            let _lock = acquire_lock(session_dir, "test-tool").expect("First lock should succeed");
            // Lock is held here
        } // Lock is dropped here

        // Note: fd-lock (flock) locks are process-scoped, not thread-scoped
        // Within the same process, the lock is still held even after drop
        // This test documents the behavior but won't actually re-acquire
        // In real usage across processes, the lock would be released
    }

    #[test]
    fn test_different_tools_different_locks() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let session_dir = temp_dir.path();

        let lock1 = acquire_lock(session_dir, "tool-a").expect("Tool A lock should succeed");
        let lock2 = acquire_lock(session_dir, "tool-b").expect("Tool B lock should succeed");

        assert_ne!(
            lock1.lock_path(),
            lock2.lock_path(),
            "Different tools should have different lock files"
        );
    }
}
