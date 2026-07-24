use super::*;
use crate::session_cmds_daemon::{WaitBehavior, WaitLoopTiming, handle_session_wait_with_hooks};
use crate::session_cmds_result::{StructuredOutputOpts, handle_session_result};
use crate::test_env_lock::TEST_ENV_LOCK;
use csa_core::types::ReviewDecision;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn create_success_session(project: &Path, description: &str) -> (String, PathBuf) {
    let session =
        create_session(project, Some(description), None, Some("codex")).expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session directory");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write success completion");
    save_result(
        project,
        &session_id,
        &SessionResult {
            summary: "transport completed successfully".to_string(),
            ..make_result("success", 0)
        },
    )
    .expect("save provisional result");
    (session_id, session_dir)
}

fn wait_fails(project: &Path, session_id: &str) {
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
            panic!("terminal artifacts should short-circuit")
        },
        |session, status, exit, synthetic, _mirror| {
            completion = Some((session.to_string(), status.to_string(), exit, synthetic));
        },
    )
    .expect("wait terminal result");

    assert_eq!(exit_code, 1);
    assert_eq!(
        completion,
        Some((session_id.to_string(), "failure".to_string(), 1, false))
    );
}

fn wait_succeeds(project: &Path, session_id: &str) {
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
            panic!("terminal artifacts should short-circuit")
        },
        |session, status, exit, synthetic, _mirror| {
            completion = Some((session.to_string(), status.to_string(), exit, synthetic));
        },
    )
    .expect("wait terminal result");

    assert_eq!(exit_code, 0);
    assert_eq!(
        completion,
        Some((session_id.to_string(), "success".to_string(), 0, false))
    );
}

fn require_commit_recovery(
    commit_created: bool,
    dirty_worktree: bool,
    blocker_summary: Option<&str>,
) -> csa_session::RequireCommitRecoveryDiagnostic {
    csa_session::RequireCommitRecoveryDiagnostic {
        require_commit: true,
        sa_mode: Some(true),
        commit_created,
        dirty_worktree,
        changed_paths: Vec::new(),
        changed_paths_truncated: 0,
        termination_status: "failure".to_string(),
        exit_code: 1,
        termination_signal: None,
        kill_hint: None,
        blocker_summary: blocker_summary.map(str::to_string),
        suggested_recovery_action: "inspect_changed_paths_then_commit_or_revert".to_string(),
    }
}

fn write_matching_pass_sidecars(session_dir: &Path, session_id: &str) {
    let timestamp = chrono::Utc::now();
    csa_session::write_review_meta(
        session_dir,
        &csa_session::ReviewSessionMeta {
            session_id: session_id.to_string(),
            head_sha: "deadbeef".to_string(),
            decision: ReviewDecision::Pass.as_str().to_string(),
            verdict: "CLEAN".to_string(),
            review_mode: None,
            status_reason: None,
            routed_to: None,
            primary_failure: None,
            failure_reason: None,
            tool: "codex".to_string(),
            scope: "range:main...HEAD".to_string(),
            exit_code: 0,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 1,
            timestamp,
            diff_fingerprint: None,
            fix_convergence: None,
        },
    )
    .expect("write pass review metadata");
    let mut artifact = csa_session::ReviewVerdictArtifact::from_parts(
        session_id.to_string(),
        ReviewDecision::Pass,
        "CLEAN",
        &[],
        Vec::new(),
    );
    artifact.timestamp = timestamp;
    artifact.review_iterations = Some(1);
    artifact.fix_rounds = Some(0);
    csa_session::write_review_verdict(session_dir, &artifact).expect("write pass review verdict");
}

fn assert_pass_sidecar_mismatch(
    description: &str,
    mutate: impl FnOnce(&mut csa_session::ReviewVerdictArtifact),
) {
    let temp = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", temp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = temp.path();
    let (session_id, session_dir) = create_success_session(project, description);
    write_matching_pass_sidecars(&session_dir, &session_id);

    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let raw = std::fs::read_to_string(&verdict_path).expect("read verdict artifact");
    let mut artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&raw).expect("parse verdict artifact");
    mutate(&mut artifact);
    csa_session::write_review_verdict(&session_dir, &artifact).expect("rewrite mismatched verdict");

    wait_fails(project, &session_id);
}

fn assert_recovery_failure(
    description: &str,
    commit_created: bool,
    dirty_worktree: bool,
    blocker_summary: Option<&str>,
) {
    let temp = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", temp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = temp.path();
    let (session_id, _) = create_success_session(project, description);

    let mut result = load_result(project, &session_id)
        .expect("load provisional result")
        .expect("provisional result");
    result.require_commit_recovery = Some(require_commit_recovery(
        commit_created,
        dirty_worktree,
        blocker_summary,
    ));
    save_result(project, &session_id, &result).expect("save require-commit diagnostic");

    wait_fails(project, &session_id);
    let persisted = load_result(project, &session_id)
        .expect("load repaired result")
        .expect("repaired result");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 1);
}

#[test]
fn issue_2825_wait_fails_require_commit_without_qualifying_commit_on_clean_tree() {
    assert_recovery_failure("require-commit-no-commit-clean", false, false, None);
}

#[test]
fn issue_2825_wait_fails_require_commit_with_commit_but_dirty_tracked_worktree() {
    assert_recovery_failure("require-commit-commit-dirty", true, true, None);
}

#[test]
fn issue_2825_wait_fails_require_commit_when_cleanliness_is_unverifiable() {
    assert_recovery_failure(
        "require-commit-unknown-cleanliness",
        true,
        false,
        Some("clean_tree_verification=git-status-probe-failed exit_code=128"),
    );
}

#[test]
fn issue_2825_wait_forces_persisted_post_exec_gate_failure_over_matching_pass_sidecars() {
    let temp = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", temp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = temp.path();
    let (session_id, session_dir) = create_success_session(project, "post-exec-gate-authority");
    write_matching_pass_sidecars(&session_dir, &session_id);

    let mut result = load_result(project, &session_id)
        .expect("load provisional result")
        .expect("provisional result");
    result.post_exec_gate = Some(csa_session::PostExecGateReport::from_redacted_gate_output(
        "just pre-commit-fast",
        1,
        "error: gate failed",
    ));
    save_result(project, &session_id, &result).expect("save optimistic gate result");

    wait_fails(project, &session_id);
    let persisted = load_result(project, &session_id)
        .expect("load repaired result")
        .expect("repaired result");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 1);
    assert!(persisted.post_exec_gate.is_some());
}

#[test]
fn issue_2825_wait_preserves_clean_meta_without_legacy_verdict_artifact() {
    let temp = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", temp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = temp.path();
    let (session_id, session_dir) = create_success_session(project, "review-meta-without-verdict");
    write_matching_pass_sidecars(&session_dir, &session_id);
    std::fs::remove_file(session_dir.join("output").join("review-verdict.json"))
        .expect("remove verdict artifact");

    wait_succeeds(project, &session_id);
}

#[test]
fn issue_2825_wait_fails_pass_artifact_without_current_metadata() {
    let temp = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", temp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = temp.path();
    let (session_id, session_dir) = create_success_session(project, "review-verdict-without-meta");
    write_matching_pass_sidecars(&session_dir, &session_id);
    std::fs::remove_file(session_dir.join("review_meta.json")).expect("remove review metadata");

    wait_fails(project, &session_id);
}

#[test]
fn issue_2825_wait_fails_mismatched_pass_sidecar_identity_and_ordering() {
    assert_pass_sidecar_mismatch("review-sidecar-identity", |artifact| {
        artifact.session_id = "other-session".to_string();
    });
    assert_pass_sidecar_mismatch("review-sidecar-ordering", |artifact| {
        artifact.timestamp -= chrono::Duration::seconds(1);
    });
}

#[test]
fn issue_2825_wait_fails_mismatched_pass_sidecar_verdict_generation_and_retry() {
    assert_pass_sidecar_mismatch("review-sidecar-decision", |artifact| {
        artifact.decision = ReviewDecision::Fail;
    });
    assert_pass_sidecar_mismatch("review-sidecar-legacy-verdict", |artifact| {
        artifact.verdict_legacy = "HAS_ISSUES".to_string();
    });
    assert_pass_sidecar_mismatch("review-sidecar-generation", |artifact| {
        artifact.review_iterations = Some(2);
    });
    assert_pass_sidecar_mismatch("review-sidecar-retry", |artifact| {
        artifact.fix_rounds = Some(1);
    });
}

#[test]
fn issue_2825_wait_repairs_stale_review_failure_only_from_matching_current_pass_sidecars() {
    let temp = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", temp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = temp.path();
    let (session_id, session_dir) = create_success_session(project, "current-pass-sidecars");

    let mut stale = csa_session::load_result(project, &session_id)
        .expect("load stale result")
        .expect("result exists");
    stale.status = "failure".to_string();
    stale.exit_code = 1;
    stale.summary = "No blocking findings in main...HEAD.".to_string();
    csa_session::save_result(project, &session_id, &stale).expect("save stale result");
    write_matching_pass_sidecars(&session_dir, &session_id);

    wait_succeeds(project, &session_id);
}

#[test]
fn issue_2825_session_result_propagates_malformed_review_sidecar_reconciliation() {
    let temp = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", temp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = temp.path();
    let (session_id, session_dir) =
        create_success_session(project, "malformed-review-sidecar-result");
    write_matching_pass_sidecars(&session_dir, &session_id);
    std::fs::write(
        session_dir.join("output").join("review-verdict.json"),
        "{ not valid review verdict json",
    )
    .expect("write malformed review verdict");

    let result = handle_session_result(
        session_id,
        false,
        Some(project.to_string_lossy().into_owned()),
        StructuredOutputOpts::default(),
    );
    assert!(
        result.is_err(),
        "malformed current review sidecar must surface"
    );
}
