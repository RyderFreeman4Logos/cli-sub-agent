use anyhow::Result;
use std::path::{Path, PathBuf};

#[cfg(feature = "parallel-tasks")]
use anyhow::Context;
#[cfg(feature = "parallel-tasks")]
use std::fs::{self, OpenOptions};
#[cfg(feature = "parallel-tasks")]
use std::io::Write;

#[cfg(feature = "parallel-tasks")]
const LOCK_DIR: &str = ".csa/tasks";

#[derive(Debug)]
pub struct TaskLock {
    lock_path: PathBuf,
}

impl TaskLock {
    /// Acquire a lock for a file path.
    ///
    /// Returns `Ok(TaskLock)` when the lock is acquired and an error when the
    /// path is already locked by another live session.
    #[cfg(feature = "parallel-tasks")]
    pub fn acquire(project_root: &Path, file_path: &Path, session_id: &str) -> Result<Self> {
        let lock_dir = project_root.join(LOCK_DIR);
        fs::create_dir_all(&lock_dir).with_context(|| {
            format!(
                "Failed to create task lock directory: {}",
                lock_dir.display()
            )
        })?;

        let hash = hash_path(file_path);
        let lock_path = lock_dir.join(format!("{hash}.lock"));

        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                writeln!(file, "{session_id}")?;
                Ok(Self { lock_path })
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let content = fs::read_to_string(&lock_path).with_context(|| {
                    format!("Failed to read task lock file: {}", lock_path.display())
                })?;
                let lock_session_id = content.trim();
                if is_session_alive(lock_session_id) {
                    anyhow::bail!(
                        "File '{}' is locked by session {}",
                        file_path.display(),
                        lock_session_id
                    );
                }

                fs::remove_file(&lock_path).with_context(|| {
                    format!("Failed to remove stale task lock: {}", lock_path.display())
                })?;
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&lock_path)
                    .with_context(|| {
                        format!("Failed to create task lock: {}", lock_path.display())
                    })?;
                writeln!(file, "{session_id}")?;
                Ok(Self { lock_path })
            }
            Err(err) => Err(err)
                .with_context(|| format!("Failed to create task lock: {}", lock_path.display())),
        }
    }

    /// No-op when the `parallel-tasks` feature is disabled.
    #[cfg(not(feature = "parallel-tasks"))]
    pub fn acquire(_project_root: &Path, _file_path: &Path, _session_id: &str) -> Result<Self> {
        Ok(Self {
            lock_path: PathBuf::new(),
        })
    }

    pub fn release(&self) {
        if self.lock_path.as_os_str().is_empty() {
            return;
        }

        #[cfg(feature = "parallel-tasks")]
        {
            let _ = fs::remove_file(&self.lock_path);
        }
    }
}

impl Drop for TaskLock {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(feature = "parallel-tasks")]
fn hash_path(path: &Path) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(feature = "parallel-tasks")]
fn is_session_alive(_session_id: &str) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "parallel-tasks")]
    #[test]
    fn acquire_release_cycle_removes_lock_on_drop() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let file_path = Path::new("src/main.rs");
        let lock_dir = project_root.path().join(LOCK_DIR);

        {
            let _lock =
                TaskLock::acquire(project_root.path(), file_path, "01TASKSESSIONA").unwrap();
            assert!(lock_dir.is_dir());
            assert_eq!(lock_file_count(&lock_dir), 1);
        }

        assert_eq!(lock_file_count(&lock_dir), 0);
    }

    #[cfg(feature = "parallel-tasks")]
    #[test]
    fn double_acquire_same_path_fails() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let file_path = Path::new("src/main.rs");

        let _lock = TaskLock::acquire(project_root.path(), file_path, "01TASKSESSIONA").unwrap();
        let err = TaskLock::acquire(project_root.path(), file_path, "01TASKSESSIONB")
            .expect_err("second acquire should fail");
        let message = err.to_string();

        assert!(message.contains("src/main.rs"), "{message}");
        assert!(message.contains("01TASKSESSIONA"), "{message}");
    }

    #[cfg(not(feature = "parallel-tasks"))]
    #[test]
    fn acquire_without_feature_is_noop_and_allows_duplicates() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let file_path = Path::new("src/main.rs");

        let _first = TaskLock::acquire(project_root.path(), file_path, "01TASKSESSIONA").unwrap();
        let _second = TaskLock::acquire(project_root.path(), file_path, "01TASKSESSIONB").unwrap();

        assert!(!project_root.path().join(".csa/tasks").exists());
    }

    #[cfg(feature = "parallel-tasks")]
    fn lock_file_count(lock_dir: &Path) -> usize {
        std::fs::read_dir(lock_dir)
            .expect("read lock dir")
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "lock"))
            .count()
    }
}
