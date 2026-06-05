use super::{
    PostExecGateApplyOptions, PostExecGateCommandExit, PostExecGateCommandOutcome,
    PostExecGateOutcome, apply_post_exec_gate_after_success_with_runner,
    execute_post_exec_gate_command, maybe_run_post_exec_gate_with_runner,
};
use crate::cli::{Cli, Commands};
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use crate::test_session_sandbox::ScopedSessionSandbox;
use clap::Parser;
use csa_config::{PostExecGateConfig, ProjectConfig, ProjectMeta, ResourcesConfig, RunConfig};
use csa_session::{
    GATE_FAILURE_LOG_REL_PATH, SessionResult, create_session_fresh, get_session_dir, load_result,
    save_result,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

/// Representative `just pre-commit` failure output: a `cargo nextest` test
/// failure under a failing `just test` recipe. Used to exercise the structured
/// surfacing path (#1726) — parsed into `failing_step` + `failing_tests`.
const SAMPLE_GATE_FAILURE_OUTPUT: &str = "\
    Compiling cli-sub-agent v0.1.0
running cargo nextest run --workspace --all-features
        PASS [   0.004s] csa-session result::tests::roundtrips
        FAIL [   0.005s] csa-session post_exec_gate_report::tests::parses
        FAIL [   1.200s] cli-sub-agent run_cmd::tests::gate_surfaces
   Summary [   1.234s] 3 tests run: 1 passed, 2 failed
error: Recipe `test` failed on line 42 with exit code 100
error: Recipe `pre-commit` failed on line 3 with exit code 100";

fn project_config_with_gate(gate: PostExecGateConfig) -> ProjectConfig {
    ProjectConfig {
        schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: chrono::Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        github: None,
        session: Default::default(),
        memory: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        hooks: Default::default(),
        run: RunConfig {
            allow_base_branch_working: false,
            writer_must_commit: false,
            post_exec_gate: gate,
        },
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

fn init_clean_git_repo(project_root: &Path) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git command failed: {:?}", args);
    };

    run(&["init", "--initial-branch", "main"]);
    run(&["config", "user.name", "CSA Test"]);
    run(&["config", "user.email", "csa-test@example.com"]);
    std::fs::write(project_root.join(".gitignore"), "cache/\nstate/\n").expect("write gitignore");
    std::fs::write(project_root.join("tracked.txt"), "initial\n").expect("write tracked file");
    run(&["add", ".gitignore", "tracked.txt"]);
    run(&["commit", "-m", "initial"]);
}

fn run_git(project_root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git command failed: {:?}", args);
}

fn create_session_at_current_head(project_root: &Path) -> String {
    create_session_fresh(
        project_root,
        Some("post-exec gate test"),
        None,
        Some("codex"),
    )
    .expect("create session")
    .meta_session_id
}

fn write_success_result_for(project_root: &Path, session_id: &str) {
    let now = chrono::Utc::now();
    let result = SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: "task completed".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        fallback_chain: None,
        gate_timeout: false,
        warnings: Vec::new(),
        raw_process_exit_code: None,
        uncommitted_changes: None,
        manager_fields: Default::default(),
    };
    save_result(project_root, session_id, &result).expect("write success result");
}

fn persisted_result(project_root: &Path, session_id: &str) -> SessionResult {
    load_result(project_root, session_id)
        .expect("load result")
        .expect("result should exist")
}

fn commit_tracked_change(project_root: &Path) {
    std::fs::write(project_root.join("tracked.txt"), "committed\n").unwrap();
    run_git(project_root, &["add", "tracked.txt"]);
    run_git(project_root, &["commit", "-m", "change tracked"]);
}

fn post_exec_options(no_post_exec_gate: bool) -> PostExecGateApplyOptions<'static> {
    PostExecGateApplyOptions {
        changed_paths: None,
        extra_env: None,
        no_post_exec_gate,
        planning_only: false,
    }
}

fn planning_post_exec_options() -> PostExecGateApplyOptions<'static> {
    PostExecGateApplyOptions {
        changed_paths: None,
        extra_env: None,
        no_post_exec_gate: false,
        planning_only: true,
    }
}

/// Seed the session's `output/summary.md` + `output/details.md` with an employee
/// "success" self-report, mimicking what `persist_structured_output` writes
/// BEFORE the gate runs. Used to prove the gate-failure banner supersedes it.
fn seed_employee_success_sections(project_root: &Path, session_id: &str) {
    let output_dir = get_session_dir(project_root, session_id)
        .expect("session dir")
        .join("output");
    std::fs::create_dir_all(&output_dir).expect("create output dir");
    std::fs::write(
        output_dir.join("summary.md"),
        "## Summary\n\nAll changes implemented and committed. Everything is clean. ✅\n",
    )
    .expect("write summary.md");
    std::fs::write(
        output_dir.join("details.md"),
        "## Details\n\nThe task succeeded; all tests pass locally.\n",
    )
    .expect("write details.md");
}

/// Assert the #1726 structured-surfacing artifacts after a nonzero-exit gate
/// failure: a non-contradictory `result.toml` summary, a populated
/// `[post_exec_gate]` table, the full `gate-failure.log`, and the banner
/// prepended to `summary.md`.
fn assert_gate_failure_surfaced(project_root: &Path, session_id: &str, result: &SessionResult) {
    // result.toml: authoritative gate verdict overrides the employee self-report.
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(!result.gate_timeout);
    assert!(
        result.warnings.is_empty(),
        "no success warning on fatal exit: {:?}",
        result.warnings
    );
    assert!(
        result.summary.starts_with("POST-EXEC GATE FAILED"),
        "summary must lead with the gate verdict, got: {}",
        result.summary
    );

    // [post_exec_gate] table with the parsed failing step + tests.
    let report = result
        .post_exec_gate
        .as_ref()
        .expect("post_exec_gate table present on gate failure");
    assert_eq!(report.exit_code, 100);
    assert_eq!(report.gate_command, "just pre-commit");
    assert_eq!(report.failing_step.as_deref(), Some("just test"));
    assert!(
        report
            .failing_tests
            .iter()
            .any(|t| t.contains("post_exec_gate_report::tests::parses")),
        "failing tests parsed from nextest output: {:?}",
        report.failing_tests
    );
    assert!(!report.output_tail.is_empty());
    assert_eq!(report.log_path, GATE_FAILURE_LOG_REL_PATH);

    // gate-failure.log: the full, unbounded gate output.
    let session_dir = get_session_dir(project_root, session_id).expect("session dir");
    let log = std::fs::read_to_string(session_dir.join(GATE_FAILURE_LOG_REL_PATH))
        .expect("gate-failure.log written");
    assert!(log.contains("FAIL ["), "log holds the full gate output");
    assert!(log.contains("Recipe `test` failed"));

    // summary.md leads with the banner; the employee "clean" claim is superseded.
    let summary_md =
        std::fs::read_to_string(session_dir.join("output/summary.md")).expect("summary.md present");
    assert!(
        summary_md
            .trim_start()
            .starts_with("> ⚠️ **POST-EXEC GATE FAILED"),
        "summary.md must lead with the gate-failure banner, got: {summary_md}"
    );
    assert!(summary_md.contains("SUPERSEDED"));
}

#[tokio::test]
#[serial_test::serial]
async fn execute_post_exec_gate_strips_inherited_csa_env() {
    let _lock = TEST_ENV_LOCK.clone().lock_owned().await;
    let project_dir = tempdir().unwrap();
    let _session_guard = ScopedEnvVarRestore::set("CSA_SESSION_ID", "01KTESTGATEENV000000000000");
    let _depth_guard = ScopedEnvVarRestore::set("CSA_DEPTH", "7");
    let _root_guard = ScopedEnvVarRestore::set("CSA_PROJECT_ROOT", project_dir.path());
    let _dir_guard = ScopedEnvVarRestore::set("CSA_SESSION_DIR", project_dir.path().join("state"));
    let _future_guard = ScopedEnvVarRestore::set("CSA_UNLISTED_GATE_ENV", "must-not-leak");

    let outcome = execute_post_exec_gate_command(
        r#"test -z "${CSA_SESSION_ID+x}" &&
           test -z "${CSA_DEPTH+x}" &&
           test -z "${CSA_PROJECT_ROOT+x}" &&
           test -z "${CSA_SESSION_DIR+x}" &&
           test -z "${CSA_UNLISTED_GATE_ENV+x}""#,
        project_dir.path(),
        30,
        None,
    )
    .await
    .expect("post-exec gate command should run");

    assert_eq!(outcome.exit, PostExecGateCommandExit::Exited(Some(0)));
}

#[tokio::test]
async fn execute_post_exec_gate_applies_build_jobs_env() {
    let project_dir = tempdir().unwrap();
    let env = HashMap::from([
        ("CARGO_BUILD_JOBS".to_string(), "1".to_string()),
        ("NEXTEST_TEST_THREADS".to_string(), "1".to_string()),
    ]);

    let outcome = execute_post_exec_gate_command(
        r#"test "$CARGO_BUILD_JOBS" = "1" &&
           test "$NEXTEST_TEST_THREADS" = "1""#,
        project_dir.path(),
        30,
        Some(env),
    )
    .await
    .expect("post-exec gate command should run");

    assert_eq!(outcome.exit, PostExecGateCommandExit::Exited(Some(0)));
}

#[tokio::test]
async fn post_exec_gate_passes_when_command_succeeds() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "changed\n").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let config = project_config_with_gate(PostExecGateConfig::default());
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Implement the fix in tracked.txt",
        Some("01TESTPOSTEXECGATEPASS0000000"),
        Some(&config),
        None,
        None,
        {
            let calls = Arc::clone(&calls);
            move |command, cwd, timeout_seconds, _extra_env| {
                let calls = Arc::clone(&calls);
                let command = command.to_string();
                let cwd = cwd.to_path_buf();
                Box::pin(async move {
                    calls.lock().unwrap().push((command, cwd, timeout_seconds));
                    Ok(PostExecGateCommandOutcome::exited(Some(0)))
                })
            }
        },
    )
    .await
    .expect("gate should pass");

    assert_eq!(outcome, PostExecGateOutcome::Passed);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "just pre-commit");
    assert_eq!(calls[0].1, project_dir.path());
    assert_eq!(calls[0].2, 1800);
}

#[tokio::test]
async fn post_exec_gate_failure_returns_structured_diagnostic() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "changed\n").unwrap();

    let config = project_config_with_gate(PostExecGateConfig {
        command: "cargo test".to_string(),
        ..Default::default()
    });
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Implement the fix in tracked.txt",
        Some("01TESTPOSTEXECGATEFAIL0000000"),
        Some(&config),
        None,
        None,
        |_command, _cwd, _timeout_seconds, _extra_env| {
            Box::pin(async { Ok(PostExecGateCommandOutcome::exited(Some(3))) })
        },
    )
    .await
    .expect("gate command should return a typed outcome");

    let PostExecGateOutcome::Failed(failure) = outcome else {
        panic!("expected typed gate failure, got {outcome:?}");
    };

    let rendered = failure.diagnostic;
    assert!(rendered.contains("csa: post-exec gate failed (exit=3)."));
    assert!(rendered.contains("gate command: cargo test"));
    assert!(rendered.contains(&format!("cwd: {}", project_dir.path().display())));
    assert!(rendered.contains("employee session: 01TESTPOSTEXECGATEFAIL0000000"));
    assert!(rendered.contains("branch: main"));
    assert!(rendered.contains("v1 gate does NOT auto-retry"));
}

#[tokio::test]
async fn post_exec_gate_skips_when_worktree_is_clean() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());

    let config = project_config_with_gate(PostExecGateConfig::default());
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Implement the fix in tracked.txt",
        Some("01TESTPOSTEXECGATESKIP0000000"),
        Some(&config),
        None,
        None,
        |_command, _cwd, _timeout_seconds, _extra_env| {
            Box::pin(async move {
                panic!("runner must not execute when worktree is clean");
            })
        },
    )
    .await
    .expect("clean worktree should skip gate");

    assert_eq!(outcome, PostExecGateOutcome::Skipped);
}

#[tokio::test]
async fn post_exec_gate_skips_review_and_debate_prompts() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "changed\n").unwrap();

    let config = project_config_with_gate(PostExecGateConfig::default());
    for prompt in ["# REVIEW:\nInspect the diff", "# DEBATE:\nArgue both sides"] {
        let outcome = maybe_run_post_exec_gate_with_runner(
            project_dir.path(),
            prompt,
            Some("01TESTPOSTEXECGATEREVIEW00000"),
            Some(&config),
            None,
            None,
            |_command, _cwd, _timeout_seconds, _extra_env| {
                Box::pin(async move {
                    panic!("runner must not execute for review/debate prompts");
                })
            },
        )
        .await
        .expect("review/debate prompts should skip gate");

        assert_eq!(outcome, PostExecGateOutcome::Skipped);
    }
}

#[tokio::test]
async fn post_exec_gate_skips_when_dirty_worktree_is_pre_existing() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    std::fs::write(
        project_dir.path().join("tracked.txt"),
        "pre-existing dirty\n",
    )
    .unwrap();

    let config = project_config_with_gate(PostExecGateConfig::default());
    let changed_paths: Vec<String> = Vec::new();
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Read files and write external test results",
        Some("01TESTPOSTEXECGATEPREEXIST000"),
        Some(&config),
        Some(&changed_paths),
        None,
        |_command, _cwd, _timeout_seconds, _extra_env| {
            Box::pin(async move {
                panic!("runner must not execute when this session changed no paths");
            })
        },
    )
    .await
    .expect("pre-existing dirty worktree should skip gate when delta is empty");

    assert_eq!(outcome, PostExecGateOutcome::Skipped);
}

#[tokio::test]
async fn post_exec_gate_runs_when_changed_paths_are_non_empty() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    std::fs::create_dir(project_dir.path().join("just-temp")).unwrap();
    std::fs::write(
        project_dir.path().join("just-temp").join("stderr.log"),
        "ice\n",
    )
    .unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let config = project_config_with_gate(PostExecGateConfig::default());
    let changed_paths = vec!["just-temp/stderr.log".to_string()];
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Run external test orchestration",
        Some("01TESTPOSTEXECGATEARTIFACT00"),
        Some(&config),
        Some(&changed_paths),
        None,
        {
            let calls = Arc::clone(&calls);
            move |command, cwd, timeout_seconds, _extra_env| {
                let calls = Arc::clone(&calls);
                let command = command.to_string();
                let cwd = cwd.to_path_buf();
                Box::pin(async move {
                    calls.lock().unwrap().push((command, cwd, timeout_seconds));
                    Ok(PostExecGateCommandOutcome::exited(Some(0)))
                })
            }
        },
    )
    .await
    .expect("explicit changed paths should run gate");

    assert_eq!(outcome, PostExecGateOutcome::Passed);
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn post_exec_gate_nonzero_committed_clean_is_fatal() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    let session_id = create_session_at_current_head(project_dir.path());
    commit_tracked_change(project_dir.path());
    write_success_result_for(project_dir.path(), &session_id);
    // Employee wrote a "clean ✅" self-report before the gate ran; the gate
    // failure must supersede it (#1726).
    seed_employee_success_sections(project_dir.path(), &session_id);

    let config = project_config_with_gate(PostExecGateConfig::default());
    let err = apply_post_exec_gate_after_success_with_runner(
        project_dir.path(),
        "Implement and commit the fix",
        Some(&session_id),
        Some(&config),
        post_exec_options(false),
        |_command, _cwd, _timeout_seconds, _extra_env| {
            Box::pin(async {
                Ok(PostExecGateCommandOutcome::exited_with(
                    Some(100),
                    SAMPLE_GATE_FAILURE_OUTPUT,
                ))
            })
        },
    )
    .await
    .expect_err("nonzero gate exit is fatal even when committed-clean");

    assert!(err.to_string().contains("post-exec gate failed"));
    let result = persisted_result(project_dir.path(), &session_id);
    assert_gate_failure_surfaced(project_dir.path(), &session_id, &result);
}

#[tokio::test]
async fn post_exec_gate_nonzero_dirty_is_fatal() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    let session_id = create_session_at_current_head(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "dirty\n").unwrap();
    write_success_result_for(project_dir.path(), &session_id);
    // No employee sections seeded: exercises the branch that CREATES summary.md
    // with the banner when the employee emitted none.

    let config = project_config_with_gate(PostExecGateConfig::default());
    apply_post_exec_gate_after_success_with_runner(
        project_dir.path(),
        "Modify tracked.txt",
        Some(&session_id),
        Some(&config),
        post_exec_options(false),
        |_command, _cwd, _timeout_seconds, _extra_env| {
            Box::pin(async {
                Ok(PostExecGateCommandOutcome::exited_with(
                    Some(100),
                    SAMPLE_GATE_FAILURE_OUTPUT,
                ))
            })
        },
    )
    .await
    .expect_err("nonzero gate exit is fatal for dirty work");

    let result = persisted_result(project_dir.path(), &session_id);
    assert_gate_failure_surfaced(project_dir.path(), &session_id, &result);
}

#[tokio::test]
async fn post_exec_gate_runner_error_overwrites_result_as_failure() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    let session_id = create_session_at_current_head(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "dirty\n").unwrap();
    write_success_result_for(project_dir.path(), &session_id);

    let config = project_config_with_gate(PostExecGateConfig::default());
    let err = apply_post_exec_gate_after_success_with_runner(
        project_dir.path(),
        "Modify tracked.txt",
        Some(&session_id),
        Some(&config),
        post_exec_options(false),
        |_command, _cwd, _timeout_seconds, _extra_env| {
            Box::pin(async { Err(anyhow::anyhow!("gate process unavailable")) })
        },
    )
    .await
    .expect_err("runner infrastructure error is fatal");

    assert!(err.to_string().contains("gate process unavailable"));
    let result = persisted_result(project_dir.path(), &session_id);
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(!result.gate_timeout);
    assert_eq!(
        result.summary,
        "could not run the post-exec gate: gate process unavailable"
    );
}

#[tokio::test]
async fn post_exec_gate_timeout_committed_clean_is_advisory() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    let session_id = create_session_at_current_head(project_dir.path());
    commit_tracked_change(project_dir.path());
    write_success_result_for(project_dir.path(), &session_id);

    let config = project_config_with_gate(PostExecGateConfig::default());
    apply_post_exec_gate_after_success_with_runner(
        project_dir.path(),
        "Implement and commit the fix",
        Some(&session_id),
        Some(&config),
        post_exec_options(false),
        |_command, _cwd, _timeout_seconds, _extra_env| {
            Box::pin(async { Ok(PostExecGateCommandOutcome::timed_out()) })
        },
    )
    .await
    .expect("timeout is advisory when work is committed-clean");

    let result = persisted_result(project_dir.path(), &session_id);
    assert_eq!(result.status, "success");
    assert_eq!(result.exit_code, 0);
    assert!(result.gate_timeout);
    assert!(result.summary.contains("task completed"));
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.contains("verification incomplete")),
        "warning should explain incomplete verification: {:?}",
        result.warnings
    );
}

#[tokio::test]
async fn post_exec_gate_timeout_dirty_is_fatal() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    let session_id = create_session_at_current_head(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "dirty\n").unwrap();
    write_success_result_for(project_dir.path(), &session_id);

    let config = project_config_with_gate(PostExecGateConfig::default());
    apply_post_exec_gate_after_success_with_runner(
        project_dir.path(),
        "Modify tracked.txt",
        Some(&session_id),
        Some(&config),
        post_exec_options(false),
        |_command, _cwd, _timeout_seconds, _extra_env| {
            Box::pin(async { Ok(PostExecGateCommandOutcome::timed_out()) })
        },
    )
    .await
    .expect_err("timeout is fatal when dirty work remains");

    let result = persisted_result(project_dir.path(), &session_id);
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(result.gate_timeout);
    assert_eq!(
        result.summary,
        "timeout left dirty/uncommitted work unverified"
    );
}

#[tokio::test]
async fn post_exec_gate_runs_planning_only_session_when_tracked_source_is_dirty() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    let session_id = create_session_at_current_head(project_dir.path());
    // A planning-mode (`--skill mktd`) run is expected to leave the tracked tree
    // clean; here it unexpectedly dirties tracked source. The gate MUST run to
    // verify the edit rather than being skipped by skill name alone (review
    // round 9 regression guard for #1819).
    std::fs::write(
        project_dir.path().join("tracked.txt"),
        "unexpected source edit\n",
    )
    .unwrap();
    write_success_result_for(project_dir.path(), &session_id);

    let calls = Arc::new(Mutex::new(0usize));
    let config = project_config_with_gate(PostExecGateConfig::default());
    apply_post_exec_gate_after_success_with_runner(
        project_dir.path(),
        "Produce an mktd plan",
        Some(&session_id),
        Some(&config),
        planning_post_exec_options(),
        {
            let calls = Arc::clone(&calls);
            move |_command, _cwd, _timeout_seconds, _extra_env| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    *calls.lock().unwrap() += 1;
                    Ok(PostExecGateCommandOutcome::exited(Some(0)))
                })
            }
        },
    )
    .await
    .expect("planning-only run with dirty tracked source should run the gate");

    assert_eq!(
        *calls.lock().unwrap(),
        1,
        "dirty tracked source must trigger the gate even in planning mode"
    );
    let result = persisted_result(project_dir.path(), &session_id);
    assert_eq!(result.status, "success");
    assert_eq!(result.exit_code, 0);
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.contains("left dirty tracked changes")),
        "planning-dirty override warning should be persisted: {:?}",
        result.warnings
    );
}

#[tokio::test]
async fn post_exec_gate_skips_planning_only_session_when_tracked_tree_is_clean() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    let session_id = create_session_at_current_head(project_dir.path());
    // Genuine plan-only run: artifacts go to session output, the tracked tree
    // stays clean. #1819's intent — such a run must NOT be failed by the code
    // commit gate (`just pre-commit` / check-chinese).
    write_success_result_for(project_dir.path(), &session_id);

    let config = project_config_with_gate(PostExecGateConfig::default());
    apply_post_exec_gate_after_success_with_runner(
        project_dir.path(),
        "Produce an mktd plan",
        Some(&session_id),
        Some(&config),
        planning_post_exec_options(),
        |_command, _cwd, _timeout_seconds, _extra_env| {
            Box::pin(async move {
                panic!("runner must not execute for a clean planning-only session");
            })
        },
    )
    .await
    .expect("clean planning-only session should not run code gate");

    let result = persisted_result(project_dir.path(), &session_id);
    assert_eq!(result.status, "success");
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "task completed");
    assert!(
        result.warnings.is_empty(),
        "clean planning-only run must not record an override warning: {:?}",
        result.warnings
    );
}

#[tokio::test]
async fn post_exec_gate_success_passes_both_cleanliness_states() {
    let config = project_config_with_gate(PostExecGateConfig::default());

    {
        let clean_dir = tempdir().unwrap();
        let _clean_sandbox = ScopedSessionSandbox::new(&clean_dir).await;
        init_clean_git_repo(clean_dir.path());
        let clean_session_id = create_session_at_current_head(clean_dir.path());
        commit_tracked_change(clean_dir.path());
        write_success_result_for(clean_dir.path(), &clean_session_id);

        apply_post_exec_gate_after_success_with_runner(
            clean_dir.path(),
            "Implement and commit the fix",
            Some(&clean_session_id),
            Some(&config),
            post_exec_options(false),
            |_command, _cwd, _timeout_seconds, _extra_env| {
                Box::pin(async { Ok(PostExecGateCommandOutcome::exited(Some(0))) })
            },
        )
        .await
        .expect("successful gate passes committed-clean work");

        let clean_result = persisted_result(clean_dir.path(), &clean_session_id);
        assert_eq!(clean_result.status, "success");
        assert_eq!(clean_result.exit_code, 0);
        assert!(!clean_result.gate_timeout);
        assert!(clean_result.warnings.is_empty());
        // #1726: the success path emits no structured failure artifacts and
        // leaves the employee summary untouched.
        assert!(
            clean_result.post_exec_gate.is_none(),
            "success path must not emit a [post_exec_gate] table"
        );
        assert_eq!(clean_result.summary, "task completed");
        assert!(
            !get_session_dir(clean_dir.path(), &clean_session_id)
                .expect("session dir")
                .join(GATE_FAILURE_LOG_REL_PATH)
                .exists(),
            "success path must not write gate-failure.log"
        );
    }

    {
        let dirty_dir = tempdir().unwrap();
        let _dirty_sandbox = ScopedSessionSandbox::new(&dirty_dir).await;
        init_clean_git_repo(dirty_dir.path());
        let dirty_session_id = create_session_at_current_head(dirty_dir.path());
        std::fs::write(dirty_dir.path().join("tracked.txt"), "dirty\n").unwrap();
        write_success_result_for(dirty_dir.path(), &dirty_session_id);

        apply_post_exec_gate_after_success_with_runner(
            dirty_dir.path(),
            "Modify tracked.txt",
            Some(&dirty_session_id),
            Some(&config),
            post_exec_options(false),
            |_command, _cwd, _timeout_seconds, _extra_env| {
                Box::pin(async { Ok(PostExecGateCommandOutcome::exited(Some(0))) })
            },
        )
        .await
        .expect("successful gate passes dirty work");

        let dirty_result = persisted_result(dirty_dir.path(), &dirty_session_id);
        assert_eq!(dirty_result.status, "success");
        assert_eq!(dirty_result.exit_code, 0);
        assert!(!dirty_result.gate_timeout);
        assert!(dirty_result.warnings.is_empty());
        assert!(
            dirty_result.post_exec_gate.is_none(),
            "success path must not emit a [post_exec_gate] table"
        );
        assert!(
            !get_session_dir(dirty_dir.path(), &dirty_session_id)
                .expect("session dir")
                .join(GATE_FAILURE_LOG_REL_PATH)
                .exists(),
            "success path must not write gate-failure.log"
        );
    }
}

#[tokio::test]
async fn run_cli_no_post_exec_gate_skips_runner_and_persists_warning() {
    let cli = Cli::try_parse_from(["csa", "run", "--no-post-exec-gate", "prompt"])
        .expect("run CLI should parse --no-post-exec-gate");
    match cli.command {
        Commands::Run {
            no_post_exec_gate, ..
        } => assert!(no_post_exec_gate),
        _ => panic!("expected run command"),
    }

    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    let session_id = create_session_at_current_head(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "dirty\n").unwrap();
    write_success_result_for(project_dir.path(), &session_id);

    let config = project_config_with_gate(PostExecGateConfig::default());
    apply_post_exec_gate_after_success_with_runner(
        project_dir.path(),
        "Modify tracked.txt",
        Some(&session_id),
        Some(&config),
        post_exec_options(true),
        |_command, _cwd, _timeout_seconds, _extra_env| {
            Box::pin(async move {
                panic!("runner must not execute when --no-post-exec-gate is set");
            })
        },
    )
    .await
    .expect("skip flag should preserve successful turn result");

    let result = persisted_result(project_dir.path(), &session_id);
    assert_eq!(result.status, "success");
    assert_eq!(result.exit_code, 0);
    assert!(!result.gate_timeout);
    assert!(
        result.warnings.iter().any(|warning| warning
            == "post-exec gate skipped by --no-post-exec-gate; external verification required"),
        "skip warning should be persisted: {:?}",
        result.warnings
    );
}

#[tokio::test]
async fn post_exec_gate_runs_when_session_introduced_changes() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "changed\n").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let config = project_config_with_gate(PostExecGateConfig::default());
    let changed_paths = vec!["tracked.txt".to_string()];
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Implement the fix in tracked.txt",
        Some("01TESTPOSTEXECGATEDELTA00000"),
        Some(&config),
        Some(&changed_paths),
        None,
        {
            let calls = Arc::clone(&calls);
            move |command, cwd, timeout_seconds, _extra_env| {
                let calls = Arc::clone(&calls);
                let command = command.to_string();
                let cwd = cwd.to_path_buf();
                Box::pin(async move {
                    calls.lock().unwrap().push((command, cwd, timeout_seconds));
                    Ok(PostExecGateCommandOutcome::exited(Some(0)))
                })
            }
        },
    )
    .await
    .expect("session-introduced changes should run gate");

    assert_eq!(outcome, PostExecGateOutcome::Passed);
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn post_exec_gate_runs_when_session_committed_changes() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    let session_id = create_session_at_current_head(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "committed\n").unwrap();
    run_git(project_dir.path(), &["add", "tracked.txt"]);
    run_git(project_dir.path(), &["commit", "-m", "change tracked"]);

    let calls = Arc::new(Mutex::new(Vec::new()));
    let config = project_config_with_gate(PostExecGateConfig::default());
    let changed_paths: Vec<String> = Vec::new();
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Implement and commit the fix",
        Some(&session_id),
        Some(&config),
        Some(&changed_paths),
        None,
        {
            let calls = Arc::clone(&calls);
            move |command, cwd, timeout_seconds, _extra_env| {
                let calls = Arc::clone(&calls);
                let command = command.to_string();
                let cwd = cwd.to_path_buf();
                Box::pin(async move {
                    calls.lock().unwrap().push((command, cwd, timeout_seconds));
                    Ok(PostExecGateCommandOutcome::exited(Some(0)))
                })
            }
        },
    )
    .await
    .expect("committed session changes should run gate");

    assert_eq!(outcome, PostExecGateOutcome::Passed);
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn post_exec_gate_runs_when_untracked_source_exists_without_changed_paths() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    std::fs::write(
        project_dir.path().join("new_source.rs"),
        "fn new_source() {}\n",
    )
    .unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let config = project_config_with_gate(PostExecGateConfig::default());
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Create a new Rust source file",
        Some("01TESTPOSTEXECGATEUNTRACKED"),
        Some(&config),
        None,
        None,
        {
            let calls = Arc::clone(&calls);
            move |command, cwd, timeout_seconds, _extra_env| {
                let calls = Arc::clone(&calls);
                let command = command.to_string();
                let cwd = cwd.to_path_buf();
                Box::pin(async move {
                    calls.lock().unwrap().push((command, cwd, timeout_seconds));
                    Ok(PostExecGateCommandOutcome::exited(Some(0)))
                })
            }
        },
    )
    .await
    .expect("untracked source changes should run gate");

    assert_eq!(outcome, PostExecGateOutcome::Passed);
    assert_eq!(calls.lock().unwrap().len(), 1);
}

/// Round-2 resource-lifecycle regression guard (#1726): a gate that exits 0
/// while leaving a BACKGROUNDED child holding stdout/stderr open must NOT wedge
/// the runner. The bounded pump drain + process-group reap make
/// `execute_post_exec_gate_command` return within `timeout_seconds + grace`, and
/// the leaked descendant is killed. Before the fix the drain awaited EOF forever
/// (the inherited pipe write-end never closes), defeating the gate timeout.
#[tokio::test]
#[cfg(unix)]
async fn execute_post_exec_gate_command_reaps_backgrounded_pipe_holder() {
    let project_dir = tempdir().unwrap();
    let sentinel = project_dir.path().join("pipe-holder-survived");
    let env = HashMap::from([(
        "GATE_TEST_SENTINEL".to_string(),
        sentinel.to_string_lossy().into_owned(),
    )]);

    // The backgrounded subshell inherits stdout/stderr (the tee pipe) and holds
    // it open while it sleeps, then WOULD create the sentinel at +6s. The leader
    // exits 0 immediately (the NORMAL-exit path). The drain grace (2s) expires
    // because the pipe stays open, triggering the process-group kill — which
    // reaps the subshell well before +6s, so the sentinel is never written.
    let command = r#"(sleep 6; : > "$GATE_TEST_SENTINEL") & exit 0"#;

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        execute_post_exec_gate_command(command, project_dir.path(), 30, Some(env)),
    )
    .await
    .expect("gate must not hang when a backgrounded child holds the pipe open")
    .expect("post-exec gate command should run");

    // The leader's own exit is still reported faithfully.
    assert_eq!(outcome.exit, PostExecGateCommandExit::Exited(Some(0)));

    // Wait past the subshell's +6s mark: a surviving holder would have created
    // the sentinel by now. Its absence proves the group kill reaped it.
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    assert!(
        !sentinel.exists(),
        "backgrounded pipe-holder must be reaped by the process-group kill, not outlive the drain"
    );
}

/// Round-2 memory bound (#1726): a gate that floods stdout far past
/// `GATE_CAPTURE_MAX_BYTES` must keep the in-memory capture bounded (no
/// unbounded slurp) while still tee-ing the full transcript to the parent. The
/// retained `captured_output` is the bounded TAIL prefixed with a truncation
/// marker.
#[tokio::test]
#[cfg(unix)]
async fn execute_post_exec_gate_command_bounds_capture_under_flood() {
    use crate::run_cmd_post_exec_gate_capture::GATE_CAPTURE_MAX_BYTES;

    let project_dir = tempdir().unwrap();
    // `head -c` bounds the producer and closes the pipe at the byte count
    // (sending `yes` SIGPIPE), so the gate exits cleanly with a transcript twice
    // the capture cap — exercising truncation without a leaked holder.
    let flood_bytes = GATE_CAPTURE_MAX_BYTES * 2;
    let command = format!("yes payload-line | head -c {flood_bytes}");

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        execute_post_exec_gate_command(&command, project_dir.path(), 30, None),
    )
    .await
    .expect("flooding gate must still terminate")
    .expect("post-exec gate command should run");

    assert_eq!(outcome.exit, PostExecGateCommandExit::Exited(Some(0)));
    assert!(
        outcome.captured_output.len() <= GATE_CAPTURE_MAX_BYTES + 256,
        "captured output must stay bounded under flood, got {} bytes",
        outcome.captured_output.len()
    );
    assert!(
        outcome
            .captured_output
            .starts_with("[csa: gate output truncated"),
        "bounded capture must disclose truncation"
    );
}
