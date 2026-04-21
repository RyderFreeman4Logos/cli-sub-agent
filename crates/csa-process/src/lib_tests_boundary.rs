//! Workspace-boundary detector tests (#981).
//!
//! Extracted from `lib_tests.rs` to keep that file under the 800-line monolith limit.

use super::*;

/// Process-wide lock for `CSA_WORKSPACE_BOUNDARY_THRESHOLD` mutation in tests.
///
/// cargo-test runs tests in parallel within a crate; env vars are process-global.
static WORKSPACE_BOUNDARY_ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

fn restore_boundary_env(original: Option<String>) {
    // SAFETY: test-scoped env mutation guarded by WORKSPACE_BOUNDARY_ENV_LOCK.
    unsafe {
        match original {
            Some(value) => std::env::set_var(WORKSPACE_BOUNDARY_THRESHOLD_ENV, value),
            None => std::env::remove_var(WORKSPACE_BOUNDARY_THRESHOLD_ENV),
        }
    }
}

// Substring used by #981 detector tests.  Deliberately assembled at runtime so
// that a child session reading THIS test file would not trip the parent CSA's
// boundary-warn hint on the raw literal too eagerly.
fn boundary_error_echo_phrase() -> String {
    // Matches is_workspace_boundary_error_line substring B.
    format!(
        "Error executing tool read_file: {} resolves {}",
        "Path not in workspace: /tmp/foo", "outside the allowed workspace directories"
    )
}

#[tokio::test]
async fn test_wait_and_capture_repeated_workspace_boundary_errors_warns_but_does_not_kill() {
    let _env_lock = WORKSPACE_BOUNDARY_ENV_LOCK
        .lock()
        .expect("boundary env lock poisoned");
    let original = std::env::var(WORKSPACE_BOUNDARY_THRESHOLD_ENV).ok();
    // SAFETY: test-scoped env mutation, restored on drop below.
    unsafe { std::env::remove_var(WORKSPACE_BOUNDARY_THRESHOLD_ENV) };

    let phrase = boundary_error_echo_phrase();
    // Feed 25 boundary lines to cross the default threshold of 20, then exit cleanly.
    let mut cmd = Command::new("bash");
    cmd.args([
        "-c",
        &format!(r#"for _ in $(seq 1 25); do echo "{phrase}"; done; echo final_marker"#),
    ]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let start = Instant::now();
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(30),
        Duration::from_secs(30),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("Failed to wait");
    let elapsed = start.elapsed();

    restore_boundary_env(original);

    // Post-#981: process is NOT killed — exit_code comes from bash completing normally.
    assert_eq!(
        result.exit_code, 0,
        "workspace boundary warn path must not force-fail the process; got {}: {}",
        result.exit_code, result.stderr_output
    );
    // Observability preserved: stderr contains the non-fatal annotation.
    assert!(
        result
            .stderr_output
            .contains("session continued (non-fatal)"),
        "stderr should carry the non-fatal annotation, got: {}",
        result.stderr_output
    );
    // Hint fired exactly once in output.
    let hint_count = result
        .output
        .matches("[csa-notice] Workspace boundary")
        .count();
    assert_eq!(
        hint_count, 1,
        "hint should fire exactly once per session, got {hint_count} in: {}",
        result.output
    );
    // Process was allowed to finish — the post-threshold echo survived.
    assert!(
        result.output.contains("final_marker"),
        "session should continue past threshold, output: {}",
        result.output
    );
    assert!(
        elapsed < Duration::from_secs(25),
        "sanity: process should run to completion, elapsed={elapsed:?}"
    );
}

#[tokio::test]
async fn test_wait_and_capture_single_workspace_boundary_error_does_not_fail_fast() {
    let _env_lock = WORKSPACE_BOUNDARY_ENV_LOCK
        .lock()
        .expect("boundary env lock poisoned");
    let original = std::env::var(WORKSPACE_BOUNDARY_THRESHOLD_ENV).ok();
    // SAFETY: test-scoped env mutation, restored below.
    unsafe { std::env::remove_var(WORKSPACE_BOUNDARY_THRESHOLD_ENV) };

    let phrase = boundary_error_echo_phrase();
    let mut cmd = Command::new("bash");
    cmd.args(["-c", &format!(r#"echo "{phrase}"; echo done"#)]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("Failed to wait");

    restore_boundary_env(original);

    assert_eq!(
        result.exit_code, 0,
        "single boundary error should not be terminal"
    );
    assert!(result.output.contains("done"));
    // Hint must NOT fire for a single hit.
    assert!(
        !result.output.contains("[csa-notice] Workspace boundary"),
        "hint must not fire on a single hit, got: {}",
        result.output
    );
}

#[tokio::test]
async fn test_wait_and_capture_env_override_lowers_threshold() {
    let _env_lock = WORKSPACE_BOUNDARY_ENV_LOCK
        .lock()
        .expect("boundary env lock poisoned");
    let original = std::env::var(WORKSPACE_BOUNDARY_THRESHOLD_ENV).ok();
    // SAFETY: test-scoped env mutation, restored below.
    unsafe { std::env::set_var(WORKSPACE_BOUNDARY_THRESHOLD_ENV, "2") };

    let phrase = boundary_error_echo_phrase();
    let mut cmd = Command::new("bash");
    cmd.args([
        "-c",
        &format!(r#"echo "{phrase}"; echo "{phrase}"; echo done"#),
    ]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("Failed to wait");

    restore_boundary_env(original);

    assert_eq!(result.exit_code, 0, "warn path must not kill");
    let hint_count = result
        .output
        .matches("[csa-notice] Workspace boundary")
        .count();
    assert_eq!(
        hint_count, 1,
        "with threshold=2 and 2 hits, hint should fire exactly once; got {hint_count} in: {}",
        result.output
    );
}

#[tokio::test]
async fn test_wait_and_capture_env_override_invalid_falls_back_to_default() {
    let _env_lock = WORKSPACE_BOUNDARY_ENV_LOCK
        .lock()
        .expect("boundary env lock poisoned");
    let original = std::env::var(WORKSPACE_BOUNDARY_THRESHOLD_ENV).ok();
    // SAFETY: test-scoped env mutation, restored below.
    unsafe { std::env::set_var(WORKSPACE_BOUNDARY_THRESHOLD_ENV, "notanumber") };

    let phrase = boundary_error_echo_phrase();
    // 3 hits — pre-#981 threshold was 3, post-#981 default is 20, invalid env must
    // fall back to default so 3 hits MUST NOT trip the hint.
    let mut cmd = Command::new("bash");
    cmd.args([
        "-c",
        &format!(r#"echo "{phrase}"; echo "{phrase}"; echo "{phrase}"; echo done"#),
    ]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("Failed to wait");

    restore_boundary_env(original);

    assert_eq!(result.exit_code, 0);
    assert!(
        !result.output.contains("[csa-notice] Workspace boundary"),
        "invalid env must fall back to default (20); 3 hits must not trip hint. output: {}",
        result.output
    );
}

#[tokio::test]
async fn test_wait_and_capture_hint_emitted_only_once_per_session() {
    let _env_lock = WORKSPACE_BOUNDARY_ENV_LOCK
        .lock()
        .expect("boundary env lock poisoned");
    let original = std::env::var(WORKSPACE_BOUNDARY_THRESHOLD_ENV).ok();
    // SAFETY: test-scoped env mutation, restored below.
    unsafe { std::env::remove_var(WORKSPACE_BOUNDARY_THRESHOLD_ENV) };

    let phrase = boundary_error_echo_phrase();
    // Feed 25 boundary lines — at default threshold 20 this crosses with 5 hits
    // to spare, but the hint must still fire exactly once.
    let mut cmd = Command::new("bash");
    cmd.args([
        "-c",
        &format!(r#"for _ in $(seq 1 25); do echo "{phrase}"; done"#),
    ]);

    let child = spawn_tool(cmd, None).await.expect("Failed to spawn");
    let result = wait_and_capture_with_idle_timeout(
        child,
        StreamMode::BufferOnly,
        Duration::from_secs(10),
        Duration::from_secs(10),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
        SpawnOptions::default(),
        None,
    )
    .await
    .expect("Failed to wait");

    restore_boundary_env(original);

    let hint_count = result
        .output
        .matches("[csa-notice] Workspace boundary")
        .count();
    assert_eq!(
        hint_count, 1,
        "hint must fire exactly once even when hits greatly exceed threshold; got {hint_count}"
    );
}
