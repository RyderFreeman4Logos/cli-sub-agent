use super::*;
use crate::session_cmds_daemon::{
    WaitBehavior, WaitLoopTiming, handle_session_wait_with_hooks, render_wait_result_summary,
};
use crate::test_env_lock::TEST_ENV_LOCK;
use std::path::Path;
use tempfile::tempdir;

fn write_clean_review_meta(session_dir: &Path, session_id: &str) {
    let meta = csa_session::state::ReviewSessionMeta {
        session_id: session_id.to_string(),
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
    csa_session::state::write_review_meta(session_dir, &meta).expect("write review meta");
}

fn write_transport_success(project: &Path, session_id: &str, session_dir: &Path) {
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write success completion packet");
    write_clean_review_meta(session_dir, session_id);
    save_result(
        project,
        session_id,
        &SessionResult {
            summary: r#"{"type":"turn.completed","usage":{"input_tokens":100,"output_tokens":25}}"#
                .to_string(),
            ..make_result("success", 0)
        },
    )
    .expect("save transport success result");
}

#[test]
fn issue_2183_wait_fails_structured_fail_summary_even_when_transport_succeeded() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-structured-fail-summary"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    write_transport_success(project, &session_id, &session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nFAIL: one high-severity security finding remains\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist structured fail summary");

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
            panic!("structured FAIL summary should short-circuit before reconcile");
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should fail the review gate");

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
    let summary = render_wait_result_summary(&session_dir, &session_id, &persisted);
    assert!(summary.contains("Review verdict: FAIL"));
    assert!(!summary.contains("Review verdict: PASS"));
    assert!(summary.contains("Summary: FAIL"));
}

#[test]
fn issue_2183_wait_preserves_structured_pass_summary_success() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-structured-pass-summary"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    write_transport_success(project, &session_id, &session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nPASS: no blocking findings remain\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist structured pass summary");

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
            panic!("structured PASS summary should short-circuit before reconcile");
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should preserve the clean review gate");

    assert_eq!(exit_code, 0);
    assert_eq!(
        emitted_completion,
        Some((session_id.clone(), "success".to_string(), 0, false))
    );
    let persisted = load_result(project, &session_id)
        .expect("load result")
        .expect("result should remain terminal");
    assert_eq!(persisted.status, "success");
    assert_eq!(persisted.exit_code, 0);
    let summary = render_wait_result_summary(&session_dir, &session_id, &persisted);
    assert!(summary.contains("Review verdict: PASS"));
    assert!(summary.contains("Summary: PASS"));
}
