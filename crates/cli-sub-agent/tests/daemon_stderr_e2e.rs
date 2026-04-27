// End-to-end tests for daemon stderr capture and daemon-completion.toml
// writing on error paths (issues #571, #573, #574).
//
// These tests exercise the *user-observable* behavior: when a daemon child
// process exits with an error, the diagnostic message MUST appear in
// stderr.log and daemon-completion.toml MUST be written even on error paths.
//
// Two scenarios:
//   A. Ok(non-zero) — handle_run returns Ok(1) without Err propagation.
//      daemon-completion.toml must still be written with exit_code=1.
//   B. Err() propagation — handle_run returns Err(e) which is caught by
//      report_daemon_error_or_exit_code (issue #574 fix).
//      stderr.log must contain the error message and daemon-completion.toml
//      must be written.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Create a [`Command`] for the built `csa` binary with HOME, XDG_STATE_HOME,
/// and XDG_CONFIG_HOME redirected to the given temp directory.
fn csa_cmd(tmp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"));
    cmd
}

/// Compute the session directory path that CSA will use for a given project
/// root and session ID when `XDG_STATE_HOME` is set.
///
/// This mirrors `csa_session::get_session_dir` logic:
///   `$XDG_STATE_HOME/cli-sub-agent/{normalized_project_path}/sessions/{session_id}/`
fn compute_session_dir(xdg_state_home: &Path, project_root: &Path, session_id: &str) -> PathBuf {
    // Canonicalize to resolve symlinks (tempdir may go through /tmp -> real path).
    let canonical = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let normalized = canonical
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', std::path::MAIN_SEPARATOR_STR);
    xdg_state_home
        .join("cli-sub-agent")
        .join(normalized)
        .join("sessions")
        .join(session_id)
}

/// Parse daemon-completion.toml and return (exit_code, status).
fn read_daemon_completion(session_dir: &Path) -> (i32, String) {
    let path = session_dir.join("daemon-completion.toml");
    let content = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "daemon-completion.toml not found at {}: {e}",
            path.display()
        )
    });
    let table: toml::Table =
        toml::from_str(&content).expect("daemon-completion.toml should be valid TOML");
    let exit_code = table["exit_code"]
        .as_integer()
        .expect("exit_code should be integer") as i32;
    let status = table["status"]
        .as_str()
        .expect("status should be string")
        .to_string();
    (exit_code, status)
}

/// Generate a ULID-like session ID for testing (not a real ULID, but formatted
/// similarly — 26 uppercase alphanumeric chars).
fn test_session_id(suffix: &str) -> String {
    // Use a fixed prefix + suffix so test output is recognizable.
    format!("01TEST{:0>20}", suffix)
}

// ---------------------------------------------------------------------------
// Scenario A: Ok(non-zero) return path
//
// When CSA_DEPTH exceeds max_depth, load_and_validate returns Ok(None) and
// handle_run returns Ok(1). This tests that daemon-completion.toml is written
// even on the non-error exit path with a non-zero exit code.
// ---------------------------------------------------------------------------

// macOS: daemon child exit-code and completion-file behaviour diverges from Linux
// due to process lifecycle differences (signal delivery, tempdir symlinks).
// Tracked in https://github.com/RyderFreeman4Logos/cli-sub-agent/issues/637
#[cfg_attr(target_os = "macos", ignore)]
#[test]
fn daemon_child_ok_nonzero_writes_completion_toml() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_root = tmp.path().join("project");
    fs::create_dir_all(&project_root).expect("create project dir");

    let xdg_state_home = tmp.path().join(".local/state");
    let session_id = test_session_id("OKPATH00000000000001");

    let session_dir = compute_session_dir(&xdg_state_home, &project_root, &session_id);
    fs::create_dir_all(&session_dir).expect("create session dir");

    // Run as daemon-child with CSA_DEPTH=100 (exceeds default max_depth=5).
    // handle_run will call load_and_validate → None → return Ok(1).
    let output = csa_cmd(tmp.path())
        .args([
            "run",
            "--daemon-child",
            "--session-id",
            &session_id,
            "--sa-mode",
            "false",
            "--no-idle-timeout",
            "--timeout",
            "1800",
            "test prompt for scenario A",
        ])
        .env("CSA_DEPTH", "100")
        .env("CSA_DAEMON_SESSION_DIR", &session_dir)
        .env("CSA_DAEMON_PROJECT_ROOT", &project_root)
        .current_dir(&project_root)
        .output()
        .expect("failed to run csa");

    // Process should exit with code 1 (not hang or crash).
    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1, got {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // daemon-completion.toml MUST exist.
    let completion_path = session_dir.join("daemon-completion.toml");
    assert!(
        completion_path.exists(),
        "daemon-completion.toml should exist at {}",
        completion_path.display()
    );

    let (exit_code, status) = read_daemon_completion(&session_dir);
    assert_eq!(exit_code, 1, "exit_code should be 1");
    // status_from_exit_code(1) = "failure" (0 = "success", 137/143 = "signal", else = "failure").
    assert_eq!(
        status, "failure",
        "status should be 'failure' for exit code 1"
    );
}

// ---------------------------------------------------------------------------
// Scenario B: Err() propagation path
//
// Using --tool with an unknown name triggers resolve_alias failure inside
// handle_run (after daemon guard is established), which returns Err.
// report_daemon_error_or_exit_code catches this, writes to stderr via
// eprintln!, then finalizes the daemon guard.
//
// This is the exact path that caused double-panic → orphaned process
// before the issue #574 fix.
// ---------------------------------------------------------------------------

// macOS: daemon child exit-code and completion-file behaviour diverges from Linux
// due to process lifecycle differences (signal delivery, tempdir symlinks).
// Tracked in https://github.com/RyderFreeman4Logos/cli-sub-agent/issues/637
#[cfg_attr(target_os = "macos", ignore)]
#[test]
fn daemon_child_err_path_captures_stderr_and_writes_completion() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_root = tmp.path().join("project");
    fs::create_dir_all(&project_root).expect("create project dir");

    let xdg_state_home = tmp.path().join(".local/state");
    let session_id = test_session_id("ERRPATH0000000000001");

    let session_dir = compute_session_dir(&xdg_state_home, &project_root, &session_id);
    fs::create_dir_all(&session_dir).expect("create session dir");

    // Use --tool bogus-tool-xyz which passes clap parsing as ToolArg::Alias
    // but fails during resolve_alias in handle_run → Err.
    // --force-ignore-tier-setting bypasses tier enforcement so the alias
    // resolution actually runs (not blocked by tier check).
    // --allow-base-branch-commit keeps this non-repo daemon stderr test focused
    // on the alias-resolution error path.
    let child = Command::new(env!("CARGO_BIN_EXE_csa"))
        .args([
            "run",
            "--daemon-child",
            "--session-id",
            &session_id,
            "--sa-mode",
            "false",
            "--force-ignore-tier-setting",
            "--tool",
            "bogus-tool-xyz",
            "--allow-base-branch-commit",
            "--no-idle-timeout",
            "--timeout",
            "1800",
            "test prompt for scenario B",
        ])
        .env("HOME", tmp.path())
        .env("XDG_STATE_HOME", &xdg_state_home)
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .env("CSA_DAEMON_SESSION_DIR", &session_dir)
        .env("CSA_DAEMON_PROJECT_ROOT", &project_root)
        .current_dir(&project_root)
        .output()
        .expect("failed to spawn csa");

    // Must exit with code 1 (not hang, not crash with signal).
    assert_eq!(
        child.status.code(),
        Some(1),
        "expected exit code 1, got {:?}\nstdout: {}\nstderr: {}",
        child.status.code(),
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr),
    );

    // --- Check daemon-completion.toml ---
    let completion_path = session_dir.join("daemon-completion.toml");
    assert!(
        completion_path.exists(),
        "daemon-completion.toml should exist at {}",
        completion_path.display()
    );

    let (exit_code, status) = read_daemon_completion(&session_dir);
    assert_eq!(exit_code, 1, "exit_code should be 1");
    assert_eq!(
        status, "failure",
        "status should be 'failure' for exit code 1"
    );

    // --- Check stderr.log ---
    // When stderr rotation installs successfully, stderr is redirected to
    // stderr.log in the session dir. The error message from
    // report_daemon_error_or_exit_code ("Error: <msg>") should be captured.
    let stderr_log_path = session_dir.join("stderr.log");
    if stderr_log_path.exists() {
        let stderr_content = fs::read_to_string(&stderr_log_path).expect("should read stderr.log");
        // The error message should contain the tool name that failed.
        assert!(
            stderr_content.contains("bogus-tool-xyz"),
            "stderr.log should contain the unknown tool name 'bogus-tool-xyz', \
             got: {stderr_content}"
        );
        assert!(
            stderr_content.contains("Error:") || stderr_content.contains("unknown tool"),
            "stderr.log should contain error indicator, got: {stderr_content}"
        );
    } else {
        // If stderr rotation did not install (e.g. session dir resolution
        // differs from expected), the error should still be on process stderr.
        let process_stderr = String::from_utf8_lossy(&child.stderr);
        assert!(
            process_stderr.contains("bogus-tool-xyz"),
            "process stderr should contain 'bogus-tool-xyz' when stderr.log \
             is not available. stderr: {process_stderr}"
        );
    }

    // --- No double-panic backtrace ---
    // Before the fix, the process would abort with a double-panic. Verify no
    // panic backtrace appears in any output.
    let all_output = format!(
        "{}{}{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr),
        fs::read_to_string(session_dir.join("stderr.log")).unwrap_or_default(),
    );
    assert!(
        !all_output.contains("panicked at"),
        "should not contain panic backtrace, got: {all_output}"
    );
    assert!(
        !all_output.contains("thread 'main' panicked"),
        "should not contain main thread panic, got: {all_output}"
    );
}

// ---------------------------------------------------------------------------
// Scenario B variant: Err() on review command
//
// Exercises the same report_daemon_error_or_exit_code path on `csa review`
// to ensure all three execution commands (run/review/debate) are covered.
// Uses --cd with a nonexistent path to trigger determine_project_root error.
// ---------------------------------------------------------------------------

#[test]
fn daemon_child_review_err_writes_completion() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_root = tmp.path().join("project");
    fs::create_dir_all(&project_root).expect("create project dir");

    let xdg_state_home = tmp.path().join(".local/state");
    let session_id = test_session_id("REVIEW0000000000001");

    // Pre-create session dir so the env-var based completion writer can find it.
    let session_dir = compute_session_dir(&xdg_state_home, &project_root, &session_id);
    fs::create_dir_all(&session_dir).expect("create session dir");

    // `csa review --daemon-child --session-id <ID> --sa-mode false --diff --cd /nonexistent`
    // check_daemon_flags succeeds (daemon_child=true, session_id present).
    // install_daemon_stderr_rotation may fail (cd=/nonexistent), which is best-effort.
    // handle_review calls determine_project_root(Some("/nonexistent")) → Err.
    // report_daemon_error_or_exit_code catches Err → eprintln! → finalize → exit(1).
    let output = Command::new(env!("CARGO_BIN_EXE_csa"))
        .args([
            "review",
            "--daemon-child",
            "--session-id",
            &session_id,
            "--sa-mode",
            "false",
            "--diff",
            "--cd",
            "/nonexistent/path/for/daemon/stderr/e2e/test",
        ])
        .env("HOME", tmp.path())
        .env("XDG_STATE_HOME", &xdg_state_home)
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .env("CSA_DAEMON_SESSION_DIR", &session_dir)
        .env("CSA_DAEMON_PROJECT_ROOT", &project_root)
        .current_dir(&project_root)
        .output()
        .expect("failed to run csa review");

    // Must exit cleanly with code 1.
    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1, got {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // daemon-completion.toml MUST exist even when the error occurred early.
    let completion_path = session_dir.join("daemon-completion.toml");
    assert!(
        completion_path.exists(),
        "daemon-completion.toml should exist at {}",
        completion_path.display()
    );

    let (exit_code, status) = read_daemon_completion(&session_dir);
    assert_eq!(exit_code, 1, "exit_code should be 1");
    assert_eq!(
        status, "failure",
        "status should be 'failure' for exit code 1"
    );

    // The error message should mention the nonexistent path.
    // It may be in stderr.log (if rotation installed before --cd resolution)
    // or in process stderr (if rotation didn't install).
    let process_stderr = String::from_utf8_lossy(&output.stderr);
    let stderr_log = fs::read_to_string(session_dir.join("stderr.log")).unwrap_or_default();
    let combined_stderr = format!("{process_stderr}{stderr_log}");

    assert!(
        combined_stderr.contains("nonexistent") || combined_stderr.contains("No such file"),
        "stderr should mention the nonexistent path. \
         process stderr: {process_stderr}\n\
         stderr.log: {stderr_log}"
    );

    // No panic backtrace.
    assert!(
        !combined_stderr.contains("panicked at"),
        "should not contain panic backtrace"
    );
}
