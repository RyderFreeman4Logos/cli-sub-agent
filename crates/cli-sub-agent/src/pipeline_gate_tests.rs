use super::*;
use serial_test::serial;
use std::collections::HashMap;

/// Set CSA_DEPTH env var for test isolation.
///
/// # Safety
/// Tests using this must be run with `#[serial]` or accept
/// that env mutations are process-global. All gate tests restore the
/// env var after use.
unsafe fn set_depth(val: &str) {
    // SAFETY: Test-only; env var is restored after each test.
    unsafe { std::env::set_var("CSA_DEPTH", val) };
}

/// Remove CSA_DEPTH env var after test.
///
/// # Safety
/// Same concurrency caveat as `set_depth`.
unsafe fn clear_depth() {
    // SAFETY: Test-only; restoring env to clean state.
    unsafe { std::env::remove_var("CSA_DEPTH") };
}

// ---------------------------------------------------------------------------
// GateResult helpers
// ---------------------------------------------------------------------------

#[test]
fn test_gate_result_skipped_is_passed() {
    let result = GateResult::skipped("test reason");
    assert!(result.skipped);
    assert!(result.passed());
    assert_eq!(result.skip_reason.as_deref(), Some("test reason"));
    assert!(result.command.is_empty());
}

#[test]
fn test_gate_result_exit_0_is_passed() {
    let result = GateResult {
        name: String::new(),
        level: 0,
        command: "true".to_string(),
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        skipped: false,
        skip_reason: None,
    };
    assert!(result.passed());
}

#[test]
fn test_gate_result_exit_1_is_not_passed() {
    let result = GateResult {
        name: String::new(),
        level: 0,
        command: "false".to_string(),
        exit_code: Some(1),
        stdout: String::new(),
        stderr: String::new(),
        skipped: false,
        skip_reason: None,
    };
    assert!(!result.passed());
}

#[test]
fn test_gate_result_no_exit_code_is_not_passed() {
    let result = GateResult {
        name: String::new(),
        level: 0,
        command: "killed".to_string(),
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        skipped: false,
        skip_reason: None,
    };
    assert!(!result.passed());
}

// ---------------------------------------------------------------------------
// evaluate_quality_gate
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn test_gate_skipped_when_csa_depth_set() {
    // SAFETY: Test-only env mutation; gate tests are not parallel-safe by nature.
    unsafe { set_depth("1") };
    let dir = tempfile::tempdir().unwrap();

    let result = evaluate_quality_gate(
        dir.path(),
        Some("echo should-not-run"),
        250,
        &GateMode::Full,
        1,
        None,
    )
    .await
    .unwrap();

    assert!(result.skipped);
    assert!(result.passed());
    assert!(result.skip_reason.as_deref().unwrap().contains("CSA_DEPTH"));

    // SAFETY: Restoring env to clean state.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
async fn test_gate_skipped_when_no_command_and_no_hooks_path() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();

    // Initialize a git repo without core.hooksPath
    tokio::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();

    let result = evaluate_quality_gate(dir.path(), None, 250, &GateMode::Full, 0, None)
        .await
        .unwrap();

    assert!(result.skipped);
    assert!(result.passed());

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
async fn test_gate_runs_explicit_command_success() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();

    let result = evaluate_quality_gate(
        dir.path(),
        Some("echo 'gate passed'"),
        250,
        &GateMode::Full,
        0,
        None,
    )
    .await
    .unwrap();

    assert!(!result.skipped);
    assert!(result.passed());
    assert_eq!(result.exit_code, Some(0));
    assert!(result.stdout.contains("gate passed"));

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
async fn test_gate_runs_explicit_command_failure() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();

    let result = evaluate_quality_gate(
        dir.path(),
        Some("echo 'lint error' >&2; exit 1"),
        250,
        &GateMode::Full,
        0,
        None,
    )
    .await
    .unwrap();

    assert!(!result.skipped);
    assert!(!result.passed());
    assert_eq!(result.exit_code, Some(1));
    assert!(result.stderr.contains("lint error"));

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
async fn test_gate_timeout() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("timeout-child-survived");
    let extra_env = HashMap::from([(
        "CSA_GATE_TIMEOUT_MARKER".to_string(),
        marker.to_string_lossy().into_owned(),
    )]);

    let result = evaluate_quality_gate(
        dir.path(),
        Some(r#"(sleep 2; touch "$CSA_GATE_TIMEOUT_MARKER") & wait"#),
        1, // 1 second timeout
        &GateMode::Full,
        0,
        Some(&extra_env),
    )
    .await
    .unwrap();

    assert!(!result.skipped);
    assert!(!result.passed());
    assert!(result.stderr.contains("timed out"));
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert!(
        !marker.exists(),
        "timeout must kill the whole process group, not leave child processes running"
    );

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
#[cfg(unix)]
async fn test_gate_preserves_output_and_reports_drain_timeout_after_leader_exits() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();
    let child_pid_path = dir.path().join("pipe-holder.pid");
    let extra_env = HashMap::from([(
        "CSA_GATE_CHILD_PID".to_string(),
        child_pid_path.to_string_lossy().into_owned(),
    )]);

    let result = match tokio::time::timeout(
        std::time::Duration::from_secs(8),
        evaluate_quality_gate(
            dir.path(),
            Some(
                r#"(sleep 60) & printf '%s\n' "$!" > "$CSA_GATE_CHILD_PID"; echo started; echo stderr-started >&2"#,
            ),
            1,
            &GateMode::Full,
            0,
            Some(&extra_env),
        ),
    )
    .await
    {
        Ok(result) => result.unwrap(),
        Err(_elapsed) => {
            kill_pid_from_file(&child_pid_path);
            // SAFETY: Restoring env before failing the test.
            unsafe { clear_depth() };
            panic!("gate must not hang when a background child holds stdout/stderr open");
        }
    };

    assert!(!result.skipped);
    assert!(!result.passed());
    assert_eq!(result.exit_code, None);
    assert!(result.stdout.contains("started"));
    assert!(result.stderr.contains("stderr-started"));
    assert!(
        result
            .stderr
            .contains("output pipe drain timed out after 2s"),
        "stderr should name the drain timeout, got: {}",
        result.stderr
    );
    assert!(
        !result.stderr.contains("Quality gate timed out after 1s"),
        "drain timeout must not be reported as the overall gate timeout"
    );

    let child_pid: i32 = std::fs::read_to_string(&child_pid_path)
        .expect("background child pid should be recorded")
        .trim()
        .parse()
        .expect("background child pid should be numeric");
    kill_pid(child_pid);

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[cfg(unix)]
fn kill_pid(pid: i32) {
    // SAFETY: Test cleanup for a PID created by this test process.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

#[cfg(unix)]
fn kill_pid_from_file(path: &std::path::Path) {
    if let Ok(pid) = std::fs::read_to_string(path).map(|content| content.trim().parse::<i32>()) {
        if let Ok(pid) = pid {
            kill_pid(pid);
        }
    }
}

#[tokio::test]
#[serial]
async fn test_gate_monitor_mode_still_runs() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();

    // Even in monitor mode, the gate runs — the mode only affects how callers
    // handle the result.
    let result =
        evaluate_quality_gate(dir.path(), Some("exit 1"), 250, &GateMode::Monitor, 0, None)
            .await
            .unwrap();

    assert!(!result.skipped);
    assert!(!result.passed());
    // In monitor mode the function still returns a GateResult; caller decides.

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
async fn test_gate_captures_stdout_and_stderr() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();

    let result = evaluate_quality_gate(
        dir.path(),
        Some("echo 'stdout-content'; echo 'stderr-content' >&2"),
        250,
        &GateMode::Full,
        0,
        None,
    )
    .await
    .unwrap();

    assert!(result.passed());
    assert!(result.stdout.contains("stdout-content"));
    assert!(result.stderr.contains("stderr-content"));

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
async fn test_gate_applies_explicit_build_jobs_env() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();
    let extra_env = crate::build_jobs_env::build_jobs_env_with(Some(1), |_| Some("99".to_string()))
        .expect("explicit build jobs should create gate env");

    let result = evaluate_quality_gate(
        dir.path(),
        Some(r#"printf '%s/%s' "$CARGO_BUILD_JOBS" "$NEXTEST_TEST_THREADS""#),
        250,
        &GateMode::Full,
        0,
        Some(&extra_env),
    )
    .await
    .unwrap();

    assert!(result.passed());
    assert_eq!(result.stdout, "1/1");

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
async fn test_gate_applies_inherited_build_jobs_env() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();
    let extra_env = crate::build_jobs_env::build_jobs_env_with(None, |key| match key {
        crate::build_jobs_env::CARGO_BUILD_JOBS_ENV => Some("3".to_string()),
        _ => None,
    })
    .expect("inherited build jobs should create gate env");

    let result = evaluate_quality_gate(
        dir.path(),
        Some(r#"printf '%s' "$CARGO_BUILD_JOBS""#),
        250,
        &GateMode::Full,
        0,
        Some(&extra_env),
    )
    .await
    .unwrap();

    assert!(result.passed());
    assert_eq!(result.stdout, "3");

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
async fn test_pipeline_applies_build_jobs_env() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();
    let extra_env = HashMap::from([
        ("CARGO_BUILD_JOBS".to_string(), "1".to_string()),
        ("NEXTEST_TEST_THREADS".to_string(), "1".to_string()),
    ]);
    let steps = vec![GateStep {
        name: "test".to_string(),
        command: r#"printf '%s/%s' "$CARGO_BUILD_JOBS" "$NEXTEST_TEST_THREADS""#.to_string(),
        level: 3,
    }];

    let result = evaluate_quality_gates(
        dir.path(),
        &steps,
        250,
        &GateMode::Full,
        0,
        Some(&extra_env),
    )
    .await
    .unwrap();

    assert!(result.passed);
    assert_eq!(result.steps[0].stdout, "1/1");

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

// ---------------------------------------------------------------------------
// detect_git_hooks_pre_commit
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn test_detect_no_hooks_path() {
    let dir = tempfile::tempdir().unwrap();
    tokio::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();

    let result = detect_git_hooks_pre_commit(dir.path()).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
#[serial]
async fn test_detect_hooks_path_with_pre_commit() {
    let dir = tempfile::tempdir().unwrap();
    tokio::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();

    // Create a hooks directory with a pre-commit script
    let hooks_dir = dir.path().join("my-hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let pre_commit = hooks_dir.join("pre-commit");
    std::fs::write(&pre_commit, "#!/bin/sh\nexit 0\n").unwrap();

    // Set core.hooksPath
    tokio::process::Command::new("git")
        .args(["config", "core.hooksPath", hooks_dir.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();

    let result = detect_git_hooks_pre_commit(dir.path()).await.unwrap();
    assert!(result.is_some());
    assert!(result.unwrap().contains("pre-commit"));
}

#[tokio::test]
#[serial]
async fn test_detect_hooks_path_without_pre_commit() {
    let dir = tempfile::tempdir().unwrap();
    tokio::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();

    // Create hooks directory but NO pre-commit script
    let hooks_dir = dir.path().join("my-hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();

    tokio::process::Command::new("git")
        .args(["config", "core.hooksPath", hooks_dir.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();

    let result = detect_git_hooks_pre_commit(dir.path()).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
#[serial]
async fn test_detect_hooks_path_relative() {
    let dir = tempfile::tempdir().unwrap();
    tokio::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();

    // Create relative hooks directory
    let hooks_dir = dir.path().join("relative-hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    std::fs::write(hooks_dir.join("pre-commit"), "#!/bin/sh\nexit 0\n").unwrap();

    // Set relative path
    tokio::process::Command::new("git")
        .args(["config", "core.hooksPath", "relative-hooks"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();

    let result = detect_git_hooks_pre_commit(dir.path()).await.unwrap();
    assert!(result.is_some());
}

// ---------------------------------------------------------------------------
// evaluate_quality_gates (multi-step pipeline)
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn test_pipeline_skipped_when_csa_depth_set() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("1") };
    let dir = tempfile::tempdir().unwrap();

    let steps = vec![GateStep {
        name: "lint".to_string(),
        command: "echo should-not-run".to_string(),
        level: 1,
    }];

    let result = evaluate_quality_gates(dir.path(), &steps, 250, &GateMode::Full, 1, None)
        .await
        .unwrap();

    assert!(result.passed);
    assert_eq!(result.steps.len(), 1);
    assert!(result.steps[0].skipped);

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
async fn test_pipeline_empty_steps_skipped() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };

    let dir = tempfile::tempdir().unwrap();
    let result = evaluate_quality_gates(dir.path(), &[], 250, &GateMode::Full, 0, None)
        .await
        .unwrap();

    assert!(result.passed);
    assert!(result.steps[0].skipped);

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
async fn test_pipeline_sequential_all_pass() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();

    let steps = vec![
        GateStep {
            name: "lint".to_string(),
            command: "echo L1-lint".to_string(),
            level: 1,
        },
        GateStep {
            name: "typecheck".to_string(),
            command: "echo L2-typecheck".to_string(),
            level: 2,
        },
        GateStep {
            name: "test".to_string(),
            command: "echo L3-test".to_string(),
            level: 3,
        },
    ];

    let result = evaluate_quality_gates(dir.path(), &steps, 250, &GateMode::Full, 0, None)
        .await
        .unwrap();

    assert!(result.passed);
    assert!(result.failed_step.is_none());
    assert_eq!(result.steps.len(), 3);
    assert_eq!(result.steps[0].name, "lint");
    assert_eq!(result.steps[0].level, 1);
    assert!(result.steps[0].stdout.contains("L1-lint"));
    assert_eq!(result.steps[1].name, "typecheck");
    assert_eq!(result.steps[1].level, 2);
    assert_eq!(result.steps[2].name, "test");
    assert_eq!(result.steps[2].level, 3);

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
async fn test_pipeline_fail_fast_on_first_failure() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();

    let steps = vec![
        GateStep {
            name: "lint".to_string(),
            command: "exit 1".to_string(),
            level: 1,
        },
        GateStep {
            name: "test".to_string(),
            command: "echo should-not-run".to_string(),
            level: 3,
        },
    ];

    let result = evaluate_quality_gates(dir.path(), &steps, 250, &GateMode::Full, 0, None)
        .await
        .unwrap();

    assert!(!result.passed);
    assert_eq!(result.failed_step.as_deref(), Some("lint"));
    // In Full mode, pipeline aborts after first failure — only 1 step ran
    assert_eq!(result.steps.len(), 1);

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
async fn test_pipeline_summary_for_review() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();

    let steps = vec![
        GateStep {
            name: "lint".to_string(),
            command: "echo ok".to_string(),
            level: 1,
        },
        GateStep {
            name: "test".to_string(),
            command: "echo ok".to_string(),
            level: 3,
        },
    ];

    let result = evaluate_quality_gates(dir.path(), &steps, 250, &GateMode::Full, 0, None)
        .await
        .unwrap();

    let summary = result.summary_for_review();
    assert!(summary.contains("Pre-review gate results:"));
    assert!(summary.contains("L1 [PASS] lint"));
    assert!(summary.contains("L3 [PASS] test"));
    assert!(summary.contains("All gates passed."));

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

// ---------------------------------------------------------------------------
// detect_lefthook
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn test_detect_lefthook_no_config() {
    // Even if lefthook binary exists, without config file detection returns None.
    let dir = tempfile::tempdir().unwrap();
    // No lefthook.yml or .lefthook.yml created

    let result = detect_lefthook(dir.path()).await;
    assert!(result.is_none(), "Should return None without config file");
}

#[tokio::test]
#[serial]
async fn test_detect_lefthook_with_dotfile_config() {
    // Test that .lefthook.yml (dotfile variant) is also detected.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".lefthook.yml"), "pre-commit:\n").unwrap();

    // Result depends on whether lefthook binary is on PATH in the test env.
    // If binary exists, should return Some; otherwise None.
    let result = detect_lefthook(dir.path()).await;
    if result.is_some() {
        assert_eq!(result.unwrap(), "lefthook run pre-commit --no-auto-install");
    }
}

#[tokio::test]
#[serial]
async fn test_evaluate_gate_lefthook_fallback() {
    // When no explicit command and no core.hooksPath, lefthook should be tried.
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();

    // Init a git repo (so core.hooksPath check doesn't error)
    tokio::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();

    // Create lefthook config. Whether the binary is on PATH varies per env
    // (CI pre-commit job installs it via mise; bare CI `Test` job does not).
    std::fs::write(
        dir.path().join("lefthook.yml"),
        "pre-commit:\n  commands:\n    lint:\n      run: echo ok\n",
    )
    .unwrap();

    let result = evaluate_quality_gate(dir.path(), None, 250, &GateMode::Full, 0, None)
        .await
        .unwrap();

    // If lefthook is NOT on PATH: gate is skipped (no gate found) -> passed.
    // If lefthook IS on PATH: gate runs `lefthook run pre-commit ...`; the exit
    // code depends on the installed lefthook version and tempdir git state,
    // so only assert the command was resolved to the lefthook invocation.
    if which::which("lefthook").is_err() {
        assert!(result.passed(), "expected skip when lefthook missing");
    } else {
        assert_eq!(
            result.command, "lefthook run pre-commit --no-auto-install",
            "expected lefthook fallback command when binary is on PATH"
        );
    }

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

use csa_config::GateStep;
