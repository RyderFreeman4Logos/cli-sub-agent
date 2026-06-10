use super::*;
use crate::session_cmds_daemon::{WaitBehavior, WaitLoopTiming, handle_session_wait_with_hooks};
use crate::test_env_lock::TEST_ENV_LOCK;
use std::path::Path;
use tempfile::tempdir;

fn write_clean_review_meta(session_dir: &Path, session_id: &str, tool: &str) {
    let meta = csa_session::state::ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: "deadbeef".to_string(),
        decision: csa_core::types::ReviewDecision::Pass.as_str().to_string(),
        verdict: "CLEAN".to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: tool.to_string(),
        scope: "range:main...HEAD".to_string(),
        exit_code: 0,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        review_mode: None,
        fix_convergence: None,
    };
    csa_session::state::write_review_meta(session_dir, &meta).expect("write review_meta.json");
}

fn assert_wait_terminal(
    project: &Path,
    session_id: &str,
    expected_status: &str,
    expected_exit: i32,
    label: &str,
) {
    let exit_code = handle_session_wait_with_hooks(
        session_id.to_string(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("{label}: reconcile should not be reached");
        },
        |_sid, _status, _exit_code, _synthetic, _mirror| {},
    )
    .unwrap_or_else(|error| panic!("{label}: wait failed: {error}"));
    assert_eq!(exit_code, expected_exit, "{label}: exit code mismatch");
    let result = load_result(project, session_id)
        .unwrap_or_else(|error| panic!("{label}: load_result failed: {error}"))
        .unwrap_or_else(|| panic!("{label}: result missing"));
    assert_eq!(result.status, expected_status, "{label}: status mismatch");
}

#[test]
fn issue_1978_wait_uses_generated_fail_verdict_for_blocking_summary_without_fail_token() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-generated-blocking-summary-review-verdict"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write stale success completion packet");
    save_result(
        project,
        &session_id,
        &SessionResult {
            summary: "One blocking correctness finding was found in csa review.".to_string(),
            ..make_result("success", 0)
        },
    )
    .expect("save stale success result");
    let output_dir = session_dir.join("output");
    std::fs::create_dir_all(&output_dir).expect("create output dir");
    std::fs::write(output_dir.join("findings.toml"), "findings = []\n")
        .expect("write empty findings.toml");
    std::fs::write(
        output_dir.join("full.md"),
        "<!-- CSA:SECTION:summary -->\nOne blocking correctness finding was found in csa review --session 01KTMDAQM18XK6R7DDA0ZP6C57 --fix tool selection.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("write canonical review output");

    let meta = csa_session::state::ReviewSessionMeta {
        session_id: session_id.clone(),
        head_sha: "deadbeef".to_string(),
        decision: csa_core::types::ReviewDecision::Pass.as_str().to_string(),
        verdict: "CLEAN".to_string(),
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
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        review_mode: None,
        fix_convergence: None,
    };
    crate::review_cmd::persist_review_verdict_for_tests(project, &meta, &[], Vec::new());
    let generated_verdict: csa_session::ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json"))
            .expect("read generated verdict"),
    )
    .expect("parse generated verdict");
    assert_eq!(
        generated_verdict.decision,
        csa_core::types::ReviewDecision::Fail,
        "a blocking correctness finding summary must be the canonical review-verdict decision"
    );

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("generated failed review verdict should short-circuit before reconcile");
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should sync generated failed review verdict");

    assert_eq!(exit_code, 1);
    assert_eq!(
        emitted_completion,
        Some((session_id.clone(), "failure".to_string(), 1, false))
    );
    let persisted = load_result(project, &session_id)
        .expect("load result")
        .expect("result should remain terminal");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 1);
}

#[test]
fn issue_2002_found_none_does_not_trigger_blocking_signal() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-found-none-not-blocking"),
        None,
        Some("gemini-cli"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write completion");
    write_clean_review_meta(&session_dir, &session_id, "gemini-cli");
    let summary =
        "I checked for high severity issues and found none. No critical bugs reported previously.";
    save_result(
        project,
        &session_id,
        &SessionResult {
            summary: summary.to_string(),
            tool: "gemini-cli".to_string(),
            ..make_result("success", 0)
        },
    )
    .expect("save result");

    assert_wait_terminal(
        project,
        &session_id,
        "success",
        0,
        "found-none/reported-previously must not trigger blocking signal",
    );
}
