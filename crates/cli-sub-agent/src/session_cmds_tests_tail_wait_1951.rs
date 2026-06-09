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
    csa_session::state::write_review_meta(session_dir, &meta).expect("write review meta");
}

fn assert_wait_terminal(
    project: &Path,
    session_id: &str,
    expected_status: &str,
    expected_exit_code: i32,
    reconcile_panic: &str,
) {
    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.to_string(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            panic!("{reconcile_panic}");
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should complete");

    assert_eq!(exit_code, expected_exit_code);
    assert_eq!(
        emitted_completion,
        Some((
            session_id.to_string(),
            expected_status.to_string(),
            expected_exit_code,
            false,
        ))
    );
    let persisted = load_result(project, session_id)
        .expect("load result")
        .expect("result should remain terminal");
    assert_eq!(persisted.status, expected_status);
    assert_eq!(persisted.exit_code, expected_exit_code);
}

#[test]
fn handle_session_wait_syncs_failed_review_verdict_before_printing_result() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-failed-review-verdict"),
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
    let stale_success = SessionResult {
        summary: "review found blocking issues".to_string(),
        ..make_result("success", 0)
    };
    save_result(project, &session_id, &stale_success).expect("save stale success result");
    let failed_verdict = csa_session::ReviewVerdictArtifact::from_parts(
        session_id.clone(),
        csa_core::types::ReviewDecision::Fail,
        "HAS_ISSUES",
        &[],
        Vec::new(),
    );
    csa_session::write_review_verdict(&session_dir, &failed_verdict)
        .expect("write failed review verdict");

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
            panic!("review verdict refresh should short-circuit before reconcile");
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should sync failed review verdict");

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
fn issue_1971_wait_uses_generated_fail_verdict_for_pass_result() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-generated-fail-review-verdict"),
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
            summary: "FAIL: one blocking test reliability regression found".to_string(),
            ..make_result("success", 0)
        },
    )
    .expect("save stale success result");
    std::fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nFAIL: one blocking test reliability regression found in the new provider-error failover coverage.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist review summary");

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
        "a blocking FAIL summary must be the canonical review-verdict decision"
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
fn issue_1990_wait_fails_fail_prefixed_summary_without_review_verdict_artifact() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-fail-prefixed-summary-without-verdict"),
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
    .expect("write stale success completion packet");
    let summary = "FAIL  The PR successfully enables intra-tier failover for Codex model-scoped limits (#1985), ensuring we can fallback to other models when a specific model limit is hit. However, there is a High-severity defect that undermines the defense built for #1736.";
    save_result(
        project,
        &session_id,
        &SessionResult {
            summary: summary.to_string(),
            tool: "gemini-cli".to_string(),
            ..make_result("success", 0)
        },
    )
    .expect("save stale success result");

    assert_wait_terminal(
        project,
        &session_id,
        "failure",
        1,
        "summary-classified failure should short-circuit before reconcile",
    );
}

#[test]
fn issue_1990_wait_ignores_hyphenated_fail_prefix_without_review_verdict_artifact() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-hyphenated-fail-prefix-without-verdict"),
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
    .expect("write success completion packet");
    save_result(
        project,
        &session_id,
        &SessionResult {
            summary: "FAIL-over syntax is discussed here without a review verdict.".to_string(),
            tool: "gemini-cli".to_string(),
            ..make_result("success", 0)
        },
    )
    .expect("save success result");

    assert_wait_terminal(
        project,
        &session_id,
        "success",
        0,
        "bounded verdict prefix should not need reconcile",
    );
}

#[test]
fn issue_1990_wait_preserves_zero_count_review_summary_without_verdict_artifact() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-zero-count-review-summary-without-verdict"),
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
    .expect("write success completion packet");
    write_clean_review_meta(&session_dir, &session_id, "gemini-cli");
    let summary = "PASS: 0 high-severity issues. no high-severity findings. High severity: 0. **High severity**: 0. High severity vulnerabilities: 0. Critical severity vulnerabilities: 0. High severity issues = 0. Medium findings: 0. P1: 0. **P1**: 0. P1 findings: 0. P2 violations: 0. Blocking findings: 0.";
    save_result(
        project,
        &session_id,
        &SessionResult {
            summary: summary.to_string(),
            tool: "gemini-cli".to_string(),
            ..make_result("success", 0)
        },
    )
    .expect("save success result");

    assert_wait_terminal(
        project,
        &session_id,
        "success",
        0,
        "terminal success result should not need reconcile",
    );
}

#[test]
fn issue_1990_wait_fails_mixed_zero_and_nonzero_severity_summary() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-mixed-severity-summary-without-verdict"),
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
    .expect("write stale success completion packet");
    write_clean_review_meta(&session_dir, &session_id, "gemini-cli");
    let summary = "Summary: 0 critical issues, 1 high-severity issue remains.";
    save_result(
        project,
        &session_id,
        &SessionResult {
            summary: summary.to_string(),
            tool: "gemini-cli".to_string(),
            ..make_result("success", 0)
        },
    )
    .expect("save stale success result");

    assert_wait_terminal(
        project,
        &session_id,
        "failure",
        1,
        "summary-classified failure should short-circuit before reconcile",
    );
}

#[test]
fn issue_1990_wait_fails_bare_nonzero_p1_review_summary() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-bare-p1-review-summary-without-verdict"),
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
    .expect("write stale success completion packet");
    write_clean_review_meta(&session_dir, &session_id, "gemini-cli");
    save_result(
        project,
        &session_id,
        &SessionResult {
            summary: "Summary: P1: 1.".to_string(),
            tool: "gemini-cli".to_string(),
            ..make_result("success", 0)
        },
    )
    .expect("save stale success result");

    assert_wait_terminal(
        project,
        &session_id,
        "failure",
        1,
        "summary-classified failure should short-circuit before reconcile",
    );
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
