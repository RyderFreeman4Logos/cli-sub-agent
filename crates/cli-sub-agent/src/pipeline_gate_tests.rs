use super::*;
use serial_test::serial;

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

    let result = evaluate_quality_gate(dir.path(), None, 250, &GateMode::Full)
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

    let result =
        evaluate_quality_gate(dir.path(), Some("echo 'gate passed'"), 250, &GateMode::Full)
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

    let result = evaluate_quality_gate(
        dir.path(),
        Some("sleep 60"),
        1, // 1 second timeout
        &GateMode::Full,
    )
    .await
    .unwrap();

    assert!(!result.skipped);
    assert!(!result.passed());
    assert!(result.stderr.contains("timed out"));

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

#[tokio::test]
#[serial]
async fn test_gate_monitor_mode_still_runs() {
    // SAFETY: Test-only env mutation.
    unsafe { set_depth("0") };
    let dir = tempfile::tempdir().unwrap();

    // Even in monitor mode, the gate runs — the mode only affects how callers
    // handle the result.
    let result = evaluate_quality_gate(dir.path(), Some("exit 1"), 250, &GateMode::Monitor)
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
    )
    .await
    .unwrap();

    assert!(result.passed());
    assert!(result.stdout.contains("stdout-content"));
    assert!(result.stderr.contains("stderr-content"));

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

    let result = evaluate_quality_gates(dir.path(), &steps, 250, &GateMode::Full)
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
    let result = evaluate_quality_gates(dir.path(), &[], 250, &GateMode::Full)
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

    let result = evaluate_quality_gates(dir.path(), &steps, 250, &GateMode::Full)
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

    let result = evaluate_quality_gates(dir.path(), &steps, 250, &GateMode::Full)
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

    let result = evaluate_quality_gates(dir.path(), &steps, 250, &GateMode::Full)
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

    // Create lefthook config but no binary on PATH (likely in CI)
    std::fs::write(
        dir.path().join("lefthook.yml"),
        "pre-commit:\n  commands:\n    lint:\n      run: echo ok\n",
    )
    .unwrap();

    let result = evaluate_quality_gate(dir.path(), None, 250, &GateMode::Full)
        .await
        .unwrap();

    // If lefthook is on PATH: gate runs (lefthook run pre-commit).
    // If lefthook is NOT on PATH: gate is skipped (no gate found).
    // Either way, the function should not error.
    assert!(result.passed());

    // SAFETY: Restoring env.
    unsafe { clear_depth() };
}

use csa_config::GateStep;
