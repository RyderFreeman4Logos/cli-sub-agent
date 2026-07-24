use super::*;
use crate::session_cmds_daemon::{
    WaitBehavior, WaitLoopTiming, handle_session_wait_with_hooks, render_wait_result_summary,
};
use crate::test_env_lock::TEST_ENV_LOCK;
use csa_core::types::ReviewDecision;
use std::path::Path;
use tempfile::tempdir;

fn write_success_completion(session_dir: &Path) {
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write success completion packet");
}

fn write_review_meta(
    session_dir: &Path,
    session_id: &str,
    decision: ReviewDecision,
    verdict: &str,
    exit_code: i32,
    failure_reason: Option<&str>,
) {
    csa_session::state::write_review_meta(
        session_dir,
        &csa_session::ReviewSessionMeta {
            session_id: session_id.to_string(),
            head_sha: "deadbeef".to_string(),
            decision: decision.as_str().to_string(),
            verdict: verdict.to_string(),
            review_mode: None,
            status_reason: None,
            routed_to: None,
            primary_failure: None,
            failure_reason: failure_reason.map(str::to_string),
            tool: "codex".to_string(),
            scope: "range:main...HEAD".to_string(),
            exit_code,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 1,
            timestamp: chrono::Utc::now(),
            diff_fingerprint: None,
            fix_convergence: None,
        },
    )
    .expect("write review metadata");
}

fn wait_for_terminal_result(project: &Path, session_id: &str) -> i32 {
    let mut completion = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.to_string(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("existing terminal artifacts should short-circuit before reconciliation");
        },
        |session, status, exit, synthetic, _mirror| {
            completion = Some((session.to_string(), status.to_string(), exit, synthetic));
        },
    )
    .expect("wait should return a terminal result");

    assert_eq!(exit_code, 1);
    assert_eq!(
        completion,
        Some((session_id.to_string(), "failure".to_string(), 1, false,))
    );
    exit_code
}

fn create_success_session(project: &Path, description: &str) -> (String, std::path::PathBuf) {
    let session =
        create_session(project, Some(description), None, Some("codex")).expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session directory");
    write_success_completion(&session_dir);
    save_result(
        project,
        &session_id,
        &SessionResult {
            summary: "transport completed successfully".to_string(),
            ..make_result("success", 0)
        },
    )
    .expect("save provisional success result");
    (session_id, session_dir)
}

#[test]
fn issue_2825_wait_reconciles_late_require_commit_contract_failure() {
    let temp = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", temp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = temp.path();
    let (session_id, _session_dir) = create_success_session(project, "wait-require-commit-late");

    let mut terminal = load_result(project, &session_id)
        .expect("load provisional result")
        .expect("provisional result");
    terminal.require_commit_recovery = Some(csa_session::RequireCommitRecoveryDiagnostic {
        require_commit: true,
        sa_mode: Some(true),
        commit_created: false,
        dirty_worktree: true,
        changed_paths: vec!["src/lib.rs".to_string()],
        changed_paths_truncated: 0,
        termination_status: "failure".to_string(),
        exit_code: 1,
        termination_signal: None,
        kill_hint: None,
        blocker_summary: Some("gate=commit-policy-uncommitted".to_string()),
        suggested_recovery_action: "inspect_changed_paths_then_commit_or_revert".to_string(),
    });
    save_result(project, &session_id, &terminal).expect("persist late contract failure");

    wait_for_terminal_result(project, &session_id);
    let result = load_result(project, &session_id)
        .expect("load reconciled result")
        .expect("reconciled result");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    let summary = render_wait_result_summary(
        &get_session_dir(project, &session_id).expect("session directory"),
        &session_id,
        &result,
    );
    assert!(summary.contains("Require-commit recovery: CONTRACT FAILURE"));
    assert!(summary.contains("fork-from"));
}

#[test]
fn issue_2825_wait_fails_low_review_finding_from_authoritative_meta() {
    let temp = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", temp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = temp.path();
    let (session_id, session_dir) = create_success_session(project, "wait-low-review-finding");
    write_review_meta(
        &session_dir,
        &session_id,
        ReviewDecision::Fail,
        "HAS_ISSUES",
        1,
        Some("one LOW documentation-contract finding remains"),
    );

    wait_for_terminal_result(project, &session_id);
    let result = load_result(project, &session_id)
        .expect("load reconciled result")
        .expect("reconciled result");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
}

#[test]
fn issue_2825_wait_fails_provider_quota_uncertain_review_meta() {
    let temp = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", temp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = temp.path();
    let (session_id, session_dir) = create_success_session(project, "wait-provider-quota");
    write_review_meta(
        &session_dir,
        &session_id,
        ReviewDecision::Uncertain,
        "UNCERTAIN",
        1,
        Some("provider quota exhausted before review completed"),
    );

    wait_for_terminal_result(project, &session_id);
    let result = load_result(project, &session_id)
        .expect("load reconciled result")
        .expect("reconciled result");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
}

#[test]
fn issue_2825_wait_fails_closed_on_conflicting_review_artifact_fields() {
    let temp = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", temp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = temp.path();
    let (session_id, session_dir) =
        create_success_session(project, "wait-review-artifact-conflict");
    write_review_meta(
        &session_dir,
        &session_id,
        ReviewDecision::Uncertain,
        "UNCERTAIN",
        1,
        Some("provider quota exhausted before verdict finalization"),
    );
    let verdict = csa_session::ReviewVerdictArtifact::from_parts(
        session_id.clone(),
        ReviewDecision::Pass,
        "CLEAN",
        &[],
        Vec::new(),
    );
    csa_session::write_review_verdict(&session_dir, &verdict).expect("write review verdict");

    wait_for_terminal_result(project, &session_id);
    let result = load_result(project, &session_id)
        .expect("load reconciled result")
        .expect("reconciled result");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
}
