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
            if command == "jj --no-pager --color=never op log --ignore-working-copy --at-op=@ --no-graph -n 1 -T self.id().short(12)"
                && message.contains("mock jj unavailable")
    ));
    assert!(
        !state_path.exists(),
        "failed jj snapshot must not persist session state"
    );
}

#[test]
fn current_operation_id_uses_jj_limit_n() {
    let _guard = TEST_ENV_LOCK.lock().expect("lock env");
    let repo = tempdir().expect("repo tempdir");
    let bin_dir = tempdir().expect("bin tempdir");
    let arg_log = repo.path().join("jj-current-op-args.bin");
    let script = format!(
        "#!/bin/sh\n\
             if [ \"$3\" = \"op\" ] && [ \"$4\" = \"log\" ]; then\n\
               printf 'CALL\\0' >> \"{}\"\n\
               previous=''\n\
               has_ignore=0\n\
               has_at_op=0\n\
               has_limit=0\n\
               has_deprecated_limit=0\n\
               for arg in \"$@\"; do\n\
                 printf '%s\\0' \"$arg\" >> \"{}\"\n\
                 if [ \"$arg\" = \"--ignore-working-copy\" ]; then\n\
                   has_ignore=1\n\
                 fi\n\
                 if [ \"$arg\" = \"--at-op=@\" ]; then\n\
                   has_at_op=1\n\
                 fi\n\
                 if [ \"$previous\" = \"-n\" ] && [ \"$arg\" = \"1\" ]; then\n\
                   has_limit=1\n\
                 fi\n\
                 if [ \"$arg\" = \"-l\" ]; then\n\
                   has_deprecated_limit=1\n\
                 fi\n\
                 previous=\"$arg\"\n\
               done\n\
               if [ \"$has_ignore\" != \"1\" ] || [ \"$has_at_op\" != \"1\" ] || [ \"$has_limit\" != \"1\" ] || [ \"$has_deprecated_limit\" = \"1\" ]; then\n\
                 printf 'invalid op log args\\n' >&2\n\
                 exit 64\n\
               fi\n\
               printf 'op-stable\\n'\n\
               exit 0\n\
             fi\n\
             printf 'unexpected jj command\\n' >&2\n\
             exit 65\n",
        arg_log.display(),
        arg_log.display()
    );
    make_fake_jj(bin_dir.path(), &script);
    let _path_guard = PathGuard::prepend(bin_dir.path());
    let journal =
        JjJournal::with_state_path(repo.path(), repo.path().join("state").join(STATE_FILE_NAME));

    let operation_id = journal
        .current_operation_id()
        .expect("current operation id should use supported jj flags");

    assert_eq!(operation_id, "op-stable");
    let raw = fs::read(&arg_log).expect("read arg log");
    let parts = raw
        .split(|byte| *byte == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect::<Vec<_>>();
    let command_line = parts.join(" ");
    assert!(
        command_line.contains("op log --ignore-working-copy --at-op=@ --no-graph -n 1"),
        "op log must use jj's supported -n limit flag: {command_line}"
    );
    assert!(
        !command_line.contains(&format!(" {} {}", "-l", "1")),
        "op log must not use the deprecated jj limit flag: {command_line}"
    );
}

#[test]
fn empty_operation_id_reports_actual_jj_command() {
    let _guard = TEST_ENV_LOCK.lock().expect("lock env");
    let repo = tempdir().expect("repo tempdir");
    let bin_dir = tempdir().expect("bin tempdir");
    make_fake_jj(bin_dir.path(), "#!/bin/sh\nexit 0\n");
    let _path_guard = PathGuard::prepend(bin_dir.path());
    let journal =
        JjJournal::with_state_path(repo.path(), repo.path().join("state").join(STATE_FILE_NAME));

    let error = journal
        .current_operation_id()
        .expect_err("empty op id should report the command that produced it");

    assert!(matches!(
        error,
        JournalError::CommandFailed { ref command, ref message }
            if command == "jj --no-pager --color=never op log --ignore-working-copy --at-op=@ --no-graph -n 1 -T self.id().short(12)"
                && message == "operation id was empty"
    ));
}

#[test]
fn shell_metacharacters_in_message_are_safe() {
    let _guard = TEST_ENV_LOCK.lock().expect("lock env");
    let repo = tempdir().expect("repo tempdir");
    let bin_dir = tempdir().expect("bin tempdir");
    let arg_log = repo.path().join("jj-args.bin");
    let script = format!(
        "#!/bin/sh\n\
             if [ \"$3\" = \"op\" ]; then\n\
               printf 'op-stable\\n'\n\
               exit 0\n\
             fi\n\
             if [ \"$3\" = \"log\" ]; then\n\
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

    let journal =
        JjJournal::with_state_path(repo.path(), repo.path().join("state").join(STATE_FILE_NAME));
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

    assert!(parts.windows(6).any(|window| {
        window[0] == "CALL"
            && window[1] == "--no-pager"
            && window[2] == "--color=never"
            && window[3] == "describe"
            && window[4] == "-m"
            && window[5] == "msg;$(touch hacked)`echo no` / second line"
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
                ..Default::default()
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
    let _guard = TEST_ENV_LOCK.lock().expect("lock env");
    let repo = tempdir().expect("repo tempdir");
    let bin_dir = tempdir().expect("bin tempdir");
    let fake_jj = "#!/bin/sh\nif [ \"$3\" = \"op\" ]; then printf 'op-stable\\n'; exit 0; fi\nprintf 'ok\\n'\n";
    make_fake_jj(bin_dir.path(), fake_jj);
    let _path_guard = PathGuard::prepend(bin_dir.path());
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

#[test]
fn operation_drift_rejected_before_snapshot() {
    let _guard = TEST_ENV_LOCK.lock().expect("lock env");
    let repo = tempdir().expect("repo tempdir");
    let bin_dir = tempdir().expect("bin tempdir");
    let op_file = repo.path().join("current-op");
    fs::write(&op_file, "op-a").expect("write initial op");
    let script = format!(
        "#!/bin/sh\n\
             if [ \"$3\" = \"op\" ]; then\n\
               cat \"{}\"\n\
               printf '\\n'\n\
               exit 0\n\
             fi\n\
             printf 'ok\\n'\n",
        op_file.display()
    );
    make_fake_jj(bin_dir.path(), &script);
    let _path_guard = PathGuard::prepend(bin_dir.path());
    let journal =
        JjJournal::with_state_path(repo.path(), repo.path().join("state").join(STATE_FILE_NAME));

    let first = journal
        .snapshot_with_revision_supplier("first", |_| Ok(RevisionId::from("rev-a")))
        .expect("first snapshot should record lineage");
    assert_eq!(first, RevisionId::from("rev-a"));

    fs::write(&op_file, "op-b").expect("mutate op");
    let error = journal
        .snapshot_with_revision_supplier("second", |_| Ok(RevisionId::from("rev-b")))
        .expect_err("operation drift must fail closed");

    assert!(matches!(
        error,
        JournalError::InvalidState(ref message)
            if message.contains("jj operation drift detected")
                && message.contains("op-a")
                && message.contains("op-b")
    ));
    assert_eq!(
        journal
            .session_start_revision()
            .expect("state read should succeed"),
        Some(RevisionId::from("rev-a"))
    );
}

#[test]
fn read_only_operation_check_does_not_report_working_copy_snapshot_as_drift() {
    let _guard = TEST_ENV_LOCK.lock().expect("lock env");
    let repo = tempdir().expect("repo tempdir");
    let bin_dir = tempdir().expect("bin tempdir");
    let mutating_op_file = repo.path().join("mutating-op");
    fs::write(&mutating_op_file, "op-a").expect("write initial op");
    let script = format!(
        "#!/bin/sh\n\
             if [ \"$3\" = \"op\" ]; then\n\
               has_ignore=0\n\
               has_at_op=0\n\
               for arg in \"$@\"; do\n\
                 if [ \"$arg\" = \"--ignore-working-copy\" ]; then\n\
                   has_ignore=1\n\
                 fi\n\
                 if [ \"$arg\" = \"--at-op=@\" ]; then\n\
                   has_at_op=1\n\
                 fi\n\
               done\n\
               if [ \"$has_ignore\" = \"1\" ] && [ \"$has_at_op\" = \"1\" ]; then\n\
                 printf 'op-a\\n'\n\
               else\n\
                 cat \"{}\"\n\
                 printf '\\n'\n\
               fi\n\
               exit 0\n\
             fi\n\
             printf 'ok\\n'\n",
        mutating_op_file.display()
    );
    make_fake_jj(bin_dir.path(), &script);
    let _path_guard = PathGuard::prepend(bin_dir.path());
    let journal =
        JjJournal::with_state_path(repo.path(), repo.path().join("state").join(STATE_FILE_NAME));

    let first = journal
        .snapshot_with_revision_supplier("first", |_| Ok(RevisionId::from("rev-a")))
        .expect("first snapshot should record lineage");
    assert_eq!(first, RevisionId::from("rev-a"));

    fs::write(&mutating_op_file, "op-b")
        .expect("simulate jj auto-snapshot after working-copy edit");
    let second = journal
        .snapshot_with_revision_supplier("second", |_| Ok(RevisionId::from("rev-b")))
        .expect("read-only op check must not report working-copy snapshot as drift");

    assert_eq!(second, RevisionId::from("rev-b"));
    assert_eq!(
        journal
            .read_state()
            .expect("state read should succeed")
            .expect("state should be persisted")
            .last_operation_id,
        Some("op-a".to_string())
    );
}
