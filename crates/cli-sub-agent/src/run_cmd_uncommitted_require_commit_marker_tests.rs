use super::*;

fn with_valid_sandbox_failure_marker(
    test: impl FnOnce(&Path, &str, &Path, &mut ScopedSessionSandbox),
) {
    let (temp, mut sandbox) = init_repo_with_initial_commit();
    let root = temp.path();
    std::fs::write(root.join("tracked.txt"), "staged but not committed\n")
        .expect("write staged change");
    run_git(root, &["add", "tracked.txt"]);
    let head = git_capture(root, &["rev-parse", "HEAD"]);
    let staged_tree = git_capture(root, &["write-tree"]);
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let session_dir = csa_session::get_session_dir(root, &session.meta_session_id)
        .expect("session dir should resolve");
    let marker = session_dir.join(csa_hooks::git_guard::SANDBOX_COMMIT_FAILURE_MARKER_FILE);
    std::fs::write(
        &marker,
        format!("{head} {staged_tree} args env config hooks\n"),
    )
    .expect("write valid sandbox commit marker");

    test(root, &session.meta_session_id, &marker, &mut sandbox);

    assert_eq!(
        git_capture(root, &["write-tree"]),
        staged_tree,
        "marker rejection must preserve the staged index"
    );
}

#[test]
fn sandbox_commit_failure_marker_valid_nonmatch_is_clear() {
    with_valid_sandbox_failure_marker(|root, session_id, marker, _sandbox| {
        let valid_record = std::fs::read_to_string(marker).expect("read valid marker");
        let (_, rest) = valid_record.split_once(' ').expect("valid marker fields");
        std::fs::write(marker, format!("{} {rest}", "0".repeat(40)))
            .expect("write nonmatching marker");

        assert!(matches!(
            require_commit::sandbox_commit_failure_matches(root, session_id),
            Ok(false)
        ));
    });
}

#[test]
fn require_commit_recovery_rejects_multi_record_marker_as_probe_uncertainty() {
    with_valid_sandbox_failure_marker(|root, session_id, marker, _sandbox| {
        let valid_record = std::fs::read_to_string(marker).expect("read valid marker");
        std::fs::write(marker, format!("{valid_record}trailing record\n"))
            .expect("write multi-record marker");
        csa_session::save_result(root, session_id, &session_result("success", 0))
            .expect("save session result");
        let mut execution = csa_process::ExecutionResult {
            exit_code: 0,
            summary: "writer claimed success".to_string(),
            ..Default::default()
        };

        record_writer_uncommitted_changes_with_config(
            root,
            Some(session_id),
            &mut execution,
            WriterUncommittedRecord {
                sa_mode: false,
                require_commit: true,
                changed_paths: Some(&["tracked.txt".to_string()]),
                commit_created: None,
                large_diff_config: &RunLargeDiffWarningConfig::default(),
            },
        );

        let loaded = csa_session::load_result(root, session_id)
            .expect("load result")
            .expect("persisted result");
        let recovery = loaded
            .require_commit_recovery
            .expect("probe uncertainty recovery");
        let expected_reason = require_commit::contract_failure_reason(
            require_commit::SandboxHookProbeState::Uncertain("probe-failed"),
        );
        assert_eq!(execution.summary, expected_reason);
        assert_eq!(loaded.summary, expected_reason);
        assert_eq!(
            recovery.suggested_recovery_action,
            REQUIRE_COMMIT_SANDBOX_HOOK_PROBE_RECOVERY_ACTION
        );
        assert!(
            recovery
                .blocker_summary
                .as_deref()
                .is_some_and(|summary| summary.contains("sandbox_hook_probe="))
        );
    });
}

#[test]
fn require_commit_result_summary_cannot_spoof_probe_uncertainty() {
    let (temp, _sandbox) = init_repo_with_initial_commit();
    let root = temp.path();
    std::fs::write(root.join("tracked.txt"), "dirty\n").expect("write dirty tracked file");
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let mut persisted = session_result("success", 0);
    persisted.summary = "writer prose: sandbox_hook_probe=spoofed".to_string();
    csa_session::save_result(root, &session.meta_session_id, &persisted)
        .expect("save session result");
    let mut execution = csa_process::ExecutionResult {
        exit_code: 0,
        summary: persisted.summary.clone(),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: true,
            changed_paths: Some(&["tracked.txt".to_string()]),
            commit_created: Some(false),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    let loaded = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("persisted result");
    let recovery = loaded
        .require_commit_recovery
        .expect("require-commit recovery");
    assert_eq!(loaded.summary, REQUIRE_COMMIT_REASON);
    assert_eq!(
        recovery.suggested_recovery_action,
        REQUIRE_COMMIT_RECOVERY_ACTION
    );
}

#[test]
fn sandbox_commit_failure_marker_rejects_fifo_in_bounded_time() {
    use std::os::unix::ffi::OsStrExt;
    use std::time::Duration;

    with_valid_sandbox_failure_marker(|root, session_id, marker, _sandbox| {
        std::fs::remove_file(marker).expect("remove regular marker");
        let marker_c = std::ffi::CString::new(marker.as_os_str().as_bytes()).expect("marker path");
        // SAFETY: marker_c is a valid, NUL-terminated path owned for this call.
        assert_eq!(unsafe { libc::mkfifo(marker_c.as_ptr(), 0o600) }, 0);

        let root = root.to_path_buf();
        let session_id = session_id.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(require_commit::sandbox_commit_failure_matches(
                &root,
                &session_id,
            ));
        });
        let matched = rx
            .recv_timeout(Duration::from_millis(500))
            .expect("FIFO marker inspection must return within 500ms");
        assert!(matched.is_err(), "FIFO marker must be uncertain");
    });
}

#[test]
fn sandbox_commit_failure_marker_rejects_symlink() {
    use std::os::unix::fs::symlink;

    with_valid_sandbox_failure_marker(|root, session_id, marker, _sandbox| {
        let target = marker.with_extension("target");
        std::fs::rename(marker, &target).expect("move valid marker to symlink target");
        symlink(&target, marker).expect("create marker symlink");

        assert!(
            require_commit::sandbox_commit_failure_matches(root, session_id).is_err(),
            "sandbox marker symlinks must be uncertain"
        );
    });
}

#[test]
fn sandbox_commit_failure_marker_rejects_oversized_sparse_file_in_bounded_time() {
    use std::time::{Duration, Instant};

    with_valid_sandbox_failure_marker(|root, session_id, marker, _sandbox| {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(marker)
            .expect("open marker");
        file.set_len(1024 * 1024)
            .expect("make oversized sparse marker");
        let started = Instant::now();

        assert!(
            require_commit::sandbox_commit_failure_matches(root, session_id).is_err(),
            "oversized sparse marker must be uncertain"
        );
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "oversized sparse marker must be rejected before reading its payload"
        );
    });
}

#[test]
fn sandbox_commit_failure_git_probe_times_out_reaps_and_preserves_index() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    with_valid_sandbox_failure_marker(|root, session_id, _marker, sandbox| {
        let fake_bin = root.join("fake-bin");
        std::fs::create_dir(&fake_bin).expect("create fake bin");
        let fake_git = fake_bin.join("git");
        std::fs::write(
            &fake_git,
            "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"${GIT_PROBE_PID_FILE}\"\nexec /usr/bin/sleep 2\n",
        )
        .expect("write hanging git");
        std::fs::set_permissions(&fake_git, std::fs::Permissions::from_mode(0o755))
            .expect("make hanging git executable");
        let pid_file = root.join("git-probe.pid");
        let original_path = std::env::var_os("PATH").unwrap_or_default();
        let mut path = fake_bin.into_os_string();
        path.push(":");
        path.push(&original_path);
        sandbox.track_env("PATH");
        sandbox.track_env("GIT_PROBE_PID_FILE");
        // SAFETY: ScopedSessionSandbox owns TEST_ENV_LOCK and restores both variables.
        unsafe {
            std::env::set_var("PATH", path);
            std::env::set_var("GIT_PROBE_PID_FILE", &pid_file);
        }
        let started = Instant::now();

        assert!(
            require_commit::sandbox_commit_failure_matches(root, session_id).is_err(),
            "timed-out Git marker probe must be uncertain"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "Git marker probe must honor its deadline"
        );
        let pid = std::fs::read_to_string(&pid_file)
            .expect("hanging Git pid")
            .trim()
            .parse::<i32>()
            .expect("numeric hanging Git pid");
        // SAFETY: signal 0 only checks the exact child PID recorded by the fixture.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "Git probe child leaked");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH),
            "Git probe child must be synchronously reaped"
        );

        // SAFETY: restore PATH before the fixture's final real-Git index check.
        unsafe {
            std::env::set_var("PATH", original_path);
            std::env::remove_var("GIT_PROBE_PID_FILE");
        }
    });
}
