//! Sidecar jj snapshot journaling.
//!
//! This module intentionally does not participate in canonical repository
//! operations. Git remains the canonical backend; jj is only used as an
//! optional sidecar journal.

use csa_core::vcs::{JournalError, RevisionId, SnapshotJournal};
use csa_lock::acquire_project_resource_lock;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const SESSION_DIR_ENV: &str = "CSA_SESSION_DIR";
const STATE_FILE_NAME: &str = "jj-journal-state.json";
const MAX_SNAPSHOT_MESSAGE_LEN: usize = 4096;

#[derive(Debug, Clone)]
pub struct JjJournal {
    project_root: PathBuf,
    state_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct JournalState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_start_revision: Option<RevisionId>,
}

impl JjJournal {
    pub fn new(project_root: impl AsRef<Path>) -> Result<Self, JournalError> {
        let project_root = project_root.as_ref().to_path_buf();
        let state_path = derive_state_path()?;
        Ok(Self {
            project_root,
            state_path,
        })
    }

    #[cfg(test)]
    fn with_state_path(project_root: impl AsRef<Path>, state_path: impl AsRef<Path>) -> Self {
        Self {
            project_root: project_root.as_ref().to_path_buf(),
            state_path: state_path.as_ref().to_path_buf(),
        }
    }

    fn capture_snapshot_revision(&self, message: &str) -> Result<RevisionId, JournalError> {
        let sanitized = sanitize_snapshot_message(message)?;
        self.run_jj(["describe", "-m", sanitized.as_str()])?;
        self.run_jj(["new", "--no-edit"])?;

        let output = self.run_jj(["log", "--no-graph", "-r", "@-", "-T", "change_id"])?;
        let revision = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if revision.is_empty() {
            return Err(JournalError::CommandFailed {
                command: "jj log --no-graph -r @- -T change_id".to_string(),
                message: "snapshot revision id was empty".to_string(),
            });
        }
        Ok(RevisionId::from(revision))
    }

    fn run_jj<I, S>(&self, args: I) -> Result<std::process::Output, JournalError>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let collected: Vec<OsString> = args.into_iter().map(Into::into).collect();
        let output = Command::new("jj")
            .args(&collected)
            .current_dir(&self.project_root)
            .output()
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => JournalError::Unavailable(
                    "jj binary not found; git fallback is intentionally disabled".to_string(),
                ),
                _ => JournalError::Io(format!("failed to run jj: {err}")),
            })?;

        if !output.status.success() {
            let command = format!(
                "jj {}",
                collected
                    .iter()
                    .map(|arg| arg.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if stderr.is_empty() {
                format!("exit status {}", output.status)
            } else {
                stderr
            };
            return Err(JournalError::CommandFailed { command, message });
        }

        Ok(output)
    }

    fn read_state(&self) -> Result<Option<JournalState>, JournalError> {
        if !self.state_path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&self.state_path)?;
        let state = serde_json::from_str::<JournalState>(&raw)
            .map_err(|err| JournalError::InvalidState(err.to_string()))?;
        Ok(Some(state))
    }

    fn write_state(&self, state: &JournalState) -> Result<(), JournalError> {
        write_state_atomically(&self.state_path, state)
    }

    fn snapshot_with_revision_supplier<F>(
        &self,
        message: &str,
        revision_supplier: F,
    ) -> Result<RevisionId, JournalError>
    where
        F: FnOnce(&str) -> Result<RevisionId, JournalError>,
    {
        let sanitized = sanitize_snapshot_message(message)?;
        let truncated_reason: String = sanitized.chars().take(64).collect();
        let reason = format!("snapshot: {truncated_reason}");
        let _lock =
            acquire_project_resource_lock(&self.project_root, "jj-journal", "snapshot", &reason)
                .map_err(|err| JournalError::CommandFailed {
                    command: "acquire_project_resource_lock(jj-journal/snapshot)".to_string(),
                    message: err.to_string(),
                })?;

        let current_state = self.read_state()?.unwrap_or_default();
        let revision = revision_supplier(message)?;

        if current_state.session_start_revision.is_none() {
            self.write_state(&JournalState {
                session_start_revision: Some(revision.clone()),
            })?;
        }

        Ok(revision)
    }

    #[cfg(test)]
    fn write_state_with_fault_injection(
        &self,
        state: &JournalState,
        fail_after_write: bool,
    ) -> Result<(), JournalError> {
        write_state_atomically_inner(&self.state_path, state, fail_after_write)
    }
}

impl SnapshotJournal for JjJournal {
    fn snapshot(&self, message: &str) -> Result<RevisionId, JournalError> {
        self.snapshot_with_revision_supplier(message, |msg| self.capture_snapshot_revision(msg))
    }

    fn session_start_revision(&self) -> Result<Option<RevisionId>, JournalError> {
        Ok(self
            .read_state()?
            .and_then(|state| state.session_start_revision))
    }
}

fn sanitize_snapshot_message(message: &str) -> Result<String, JournalError> {
    if message.contains('\0') {
        return Err(JournalError::InvalidMessage(
            "message contains null byte".to_string(),
        ));
    }

    let normalized = message.replace("\r\n", "\n").replace('\r', "\n");
    let collapsed = normalized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");

    let bounded: String = collapsed.chars().take(MAX_SNAPSHOT_MESSAGE_LEN).collect();
    if bounded.is_empty() {
        return Err(JournalError::InvalidMessage(
            "message empty after sanitization".to_string(),
        ));
    }
    Ok(bounded)
}

fn derive_state_path() -> Result<PathBuf, JournalError> {
    std::env::var_os(SESSION_DIR_ENV)
        .map(|session_dir| PathBuf::from(session_dir).join(STATE_FILE_NAME))
        .ok_or_else(|| {
            JournalError::Unavailable(
                "CSA_SESSION_DIR not set; sidecar jj snapshot journal requires CSA session context"
                    .to_string(),
            )
        })
}

fn temp_state_path(path: &Path) -> Result<PathBuf, JournalError> {
    let file_name = path.file_name().ok_or_else(|| {
        JournalError::InvalidState(format!("state path has no file name: {}", path.display()))
    })?;
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(".tmp");
    Ok(path.with_file_name(tmp_name))
}

fn write_state_atomically(path: &Path, state: &JournalState) -> Result<(), JournalError> {
    write_state_atomically_inner(path, state, false)
}

fn write_state_atomically_inner(
    path: &Path,
    state: &JournalState,
    fail_after_write: bool,
) -> Result<(), JournalError> {
    let parent = path.parent().ok_or_else(|| {
        JournalError::InvalidState(format!("state path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent)?;
    let tmp_path = temp_state_path(path)?;
    let encoded = serde_json::to_vec_pretty(state)
        .map_err(|err| JournalError::InvalidState(err.to_string()))?;

    let mut tmp_file = fs::File::create(&tmp_path)?;
    tmp_file.write_all(&encoded)?;
    tmp_file.sync_all()?;
    drop(tmp_file);

    if fail_after_write {
        return Err(JournalError::Io(
            "simulated crash after temp write".to_string(),
        ));
    }

    fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::TEST_ENV_LOCK;
    use csa_core::vcs::SnapshotJournal;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use tempfile::tempdir;

    fn make_fake_jj(bin_dir: &Path, script_body: &str) -> PathBuf {
        let path = bin_dir.join("jj");
        fs::write(&path, script_body).expect("write fake jj");
        let mut perms = fs::metadata(&path).expect("stat fake jj").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod fake jj");
        path
    }

    struct PathGuard {
        original: Option<OsString>,
    }

    impl PathGuard {
        fn prepend(bin_dir: &Path) -> Self {
            let original = std::env::var_os("PATH");
            let mut paths = vec![bin_dir.to_path_buf()];
            if let Some(existing) = original.as_ref() {
                paths.extend(std::env::split_paths(existing));
            }
            let joined = std::env::join_paths(paths).expect("join PATH");
            // SAFETY: these tests hold TEST_ENV_LOCK for the full mutation window,
            // so no concurrent environment access can race with PATH changes.
            unsafe { std::env::set_var("PATH", joined) };
            Self { original }
        }
    }

    impl Drop for PathGuard {
        fn drop(&mut self) {
            match self.original.take() {
                // SAFETY: guarded by TEST_ENV_LOCK; see PathGuard::prepend().
                Some(path) => unsafe { std::env::set_var("PATH", path) },
                // SAFETY: guarded by TEST_ENV_LOCK; see PathGuard::prepend().
                None => unsafe { std::env::remove_var("PATH") },
            }
        }
    }

    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set_os(key: &'static str, value: &Path) -> Self {
            let original = std::env::var_os(key);
            // SAFETY: test-scoped env mutation is guarded by TEST_ENV_LOCK.
            unsafe { std::env::set_var(key, value) };
            Self { key, original }
        }

        fn unset(key: &'static str) -> Self {
            let original = std::env::var_os(key);
            // SAFETY: test-scoped env mutation is guarded by TEST_ENV_LOCK.
            unsafe { std::env::remove_var(key) };
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.original.take() {
                // SAFETY: test-scoped env restoration is guarded by TEST_ENV_LOCK.
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                // SAFETY: test-scoped env restoration is guarded by TEST_ENV_LOCK.
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn new_fails_when_session_dir_missing() {
        let _guard = TEST_ENV_LOCK.lock().expect("lock env");
        let _session_dir = EnvVarGuard::unset(SESSION_DIR_ENV);
        let repo = tempdir().expect("repo tempdir");

        let error = JjJournal::new(repo.path()).expect_err("new should fail without session dir");

        assert_eq!(
            error,
            JournalError::Unavailable(
                "CSA_SESSION_DIR not set; sidecar jj snapshot journal requires CSA session context"
                    .to_string(),
            )
        );
    }

    #[test]
    fn new_succeeds_when_session_dir_set() {
        let _guard = TEST_ENV_LOCK.lock().expect("lock env");
        let session_dir = tempdir().expect("session tempdir");
        let _session_dir = EnvVarGuard::set_os(SESSION_DIR_ENV, session_dir.path());
        let repo = tempdir().expect("repo tempdir");

        let journal = JjJournal::new(repo.path()).expect("new should succeed");

        assert_eq!(journal.state_path, session_dir.path().join(STATE_FILE_NAME),);
    }

    #[test]
    fn no_git_fallback_when_jj_missing() {
        let _guard = TEST_ENV_LOCK.lock().expect("lock env");
        let repo = tempdir().expect("repo tempdir");
        let bin_dir = tempdir().expect("bin tempdir");
        let fake_jj = "#!/bin/sh\nprintf 'mock jj unavailable\\n' >&2\nexit 127\n";
        make_fake_jj(bin_dir.path(), fake_jj);
        let _path_guard = PathGuard::prepend(bin_dir.path());
        let state_path = repo.path().join("state").join(STATE_FILE_NAME);

        let journal = JjJournal::with_state_path(repo.path(), &state_path);
        let error = journal
            .snapshot("snapshot me")
            .expect_err("fake jj should fail before any fallback");

        assert!(matches!(
            error,
            JournalError::CommandFailed { ref command, ref message }
                if command == "jj describe -m snapshot me"
                    && message.contains("mock jj unavailable")
        ));
        assert!(
            !state_path.exists(),
            "failed jj snapshot must not persist session state"
        );
    }

    #[test]
    fn shell_metacharacters_in_message_are_safe() {
        let _guard = TEST_ENV_LOCK.lock().expect("lock env");
        let repo = tempdir().expect("repo tempdir");
        let bin_dir = tempdir().expect("bin tempdir");
        let arg_log = repo.path().join("jj-args.bin");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"log\" ]; then\n\
               printf 'rev-from-log\\n'\n\
               exit 0\n\
             fi\n\
             printf 'CALL\\0' >> \"{}\"\n\
             for arg in \"$@\"; do\n\
               printf '%s\\0' \"$arg\" >> \"{}\"\n\
             done\n",
            arg_log.display(),
            arg_log.display()
        );
        make_fake_jj(bin_dir.path(), &script);

        let _path_guard = PathGuard::prepend(bin_dir.path());

        let journal = JjJournal::with_state_path(
            repo.path(),
            repo.path().join("state").join(STATE_FILE_NAME),
        );
        let unsafe_message = "msg;$(touch hacked)`echo no`\nsecond line";
        let revision = journal
            .snapshot(unsafe_message)
            .expect("snapshot should succeed");
        let start_revision = journal
            .session_start_revision()
            .expect("state read should succeed")
            .expect("start revision should be persisted");

        assert_eq!(revision.as_str(), "rev-from-log");
        assert_eq!(start_revision, revision);

        let raw = fs::read(&arg_log).expect("read arg log");
        let parts = raw
            .split(|byte| *byte == 0)
            .filter(|chunk| !chunk.is_empty())
            .map(|chunk| String::from_utf8_lossy(chunk).to_string())
            .collect::<Vec<_>>();

        assert!(parts.windows(4).any(|window| {
            window[0] == "CALL"
                && window[1] == "describe"
                && window[2] == "-m"
                && window[3] == "msg;$(touch hacked)`echo no` / second line"
        }));
        assert!(
            !repo.path().join("hacked").exists(),
            "metacharacters must not execute"
        );
    }

    #[test]
    fn atomic_state_write() {
        let repo = tempdir().expect("repo tempdir");
        let state_path = repo.path().join("state").join(STATE_FILE_NAME);
        let journal = JjJournal::with_state_path(repo.path(), &state_path);

        let error = journal
            .write_state_with_fault_injection(
                &JournalState {
                    session_start_revision: Some(RevisionId::from("rev-123")),
                },
                true,
            )
            .expect_err("fault injection should abort before rename");

        assert!(matches!(error, JournalError::Io(_)));
        assert!(
            !state_path.exists(),
            "final state file must be absent after pre-rename crash"
        );
        assert!(
            temp_state_path(&state_path).expect("tmp path").exists(),
            "temporary file should remain for crash observability"
        );
    }

    #[test]
    fn concurrent_snapshot_rejected_by_lock() {
        let repo = tempdir().expect("repo tempdir");
        let session_dir_a = tempdir().expect("session A tempdir");
        let session_dir_b = tempdir().expect("session B tempdir");
        let first_journal = Arc::new(JjJournal::with_state_path(
            repo.path(),
            session_dir_a.path().join(STATE_FILE_NAME),
        ));
        let second_journal =
            JjJournal::with_state_path(repo.path(), session_dir_b.path().join(STATE_FILE_NAME));
        let entered_lock = Arc::new(Barrier::new(2));
        let release_lock = Arc::new(Barrier::new(2));

        let first_snapshot_journal = Arc::clone(&first_journal);
        let first_entered = Arc::clone(&entered_lock);
        let first_release = Arc::clone(&release_lock);
        let first = thread::spawn(move || {
            first_snapshot_journal.snapshot_with_revision_supplier("first snapshot", |_| {
                first_entered.wait();
                first_release.wait();
                Ok(RevisionId::from("rev-first"))
            })
        });

        entered_lock.wait();

        let error = second_journal
            .snapshot_with_revision_supplier("second snapshot", |_| Ok(RevisionId::from("rev-2")))
            .expect_err("second snapshot should fail on lock contention");

        assert!(matches!(
            error,
            JournalError::CommandFailed { ref command, .. }
                if command.contains("acquire_project_resource_lock")
        ));

        release_lock.wait();

        let first_revision = first
            .join()
            .expect("first snapshot thread should not panic")
            .expect("first snapshot should succeed");
        assert_eq!(first_revision, RevisionId::from("rev-first"));
        assert_eq!(
            first_journal
                .session_start_revision()
                .expect("state read should succeed"),
            Some(RevisionId::from("rev-first"))
        );
    }
}
