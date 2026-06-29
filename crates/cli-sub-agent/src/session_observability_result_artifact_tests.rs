use super::*;

fn runtime_result(summary: &str, artifact_path: &str) -> SessionResult {
    let now = chrono::Utc::now();
    SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: summary.to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![SessionArtifact::new(artifact_path)],
        ..Default::default()
    }
}

fn failed_runtime_result(summary: &str) -> SessionResult {
    let now = chrono::Utc::now();
    SessionResult {
        post_exec_gate: None,
        status: "failure".to_string(),
        exit_code: 1,
        summary: summary.to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![SessionArtifact::new(csa_session::GATE_FAILURE_LOG_REL_PATH)],
        ..Default::default()
    }
}

fn unrelated_failed_runtime_result(summary: &str) -> SessionResult {
    let now = chrono::Utc::now();
    SessionResult {
        post_exec_gate: None,
        status: SessionResult::status_from_exit_code(143),
        exit_code: 143,
        summary: summary.to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: Vec::new(),
        ..Default::default()
    }
}

fn set_file_mtime_seconds_ago(path: &std::path::Path, seconds_ago: u64) {
    let target = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(seconds_ago))
        .expect("target mtime before now");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open file for mtime update");
    file.set_times(std::fs::FileTimes::new().set_modified(target))
        .expect("set file mtime");
}

struct StaleGateLogFixture {
    _temp: tempfile::TempDir,
    _state_guard: crate::test_env_lock::ScopedTestEnvVar,
    project_root: std::path::PathBuf,
    session_id: String,
    session_dir: std::path::PathBuf,
}

fn create_stale_gate_log_failure_fixture() -> StaleGateLogFixture {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_guard =
        crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", temp.path().join("state"));
    let project_root = temp.path().join("project");
    fs::create_dir_all(&project_root).expect("create project");
    let session = csa_session::create_session(
        &project_root,
        Some("stale gate failure log regression"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir =
        csa_session::get_session_dir(&project_root, &session_id).expect("session dir");
    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir).expect("create output dir");
    let gate_log_path = session_dir.join(csa_session::GATE_FAILURE_LOG_REL_PATH);
    fs::write(
        &gate_log_path,
        "running just pre-commit\nFAIL [   0.005s] old_gate::fails\n",
    )
    .expect("write stale gate failure log");
    set_file_mtime_seconds_ago(&gate_log_path, 600);
    csa_session::save_result(
        &project_root,
        &session_id,
        &unrelated_failed_runtime_result("terminated by model signal 143"),
    )
    .expect("save unrelated failure envelope");

    StaleGateLogFixture {
        _temp: temp,
        _state_guard: state_guard,
        project_root,
        session_id,
        session_dir,
    }
}

fn assert_unrelated_signal_failure_preserved(result: &SessionResult) {
    assert_eq!(result.status, "signal");
    assert_eq!(result.exit_code, 143);
    assert_eq!(result.summary, "terminated by model signal 143");
    assert!(
        result.post_exec_gate.is_none(),
        "stale gate-failure.log must not be attached to an unrelated current failure"
    );
}

fn assert_gate_log_is_diagnostic_only(result: &SessionResult) {
    let artifact = result
        .artifacts
        .iter()
        .find(|artifact| artifact.path == csa_session::GATE_FAILURE_LOG_REL_PATH)
        .expect("observed stale gate-failure.log should remain listed for diagnostics");
    assert!(
        artifact.display_only,
        "directory-scanned gate-failure.log must not prove current-result ownership"
    );
}

#[test]
fn refresh_and_repair_result_infers_gate_failure_from_log_artifact() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let _state_guard =
        crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", temp.path().join("state"));
    let project_root = temp.path().join("project");
    fs::create_dir_all(&project_root).expect("create project");
    let session = csa_session::create_session(
        &project_root,
        Some("gate failure artifact regression"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_dir =
        csa_session::get_session_dir(&project_root, &session.meta_session_id).expect("session dir");
    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir).expect("create output dir");
    fs::write(
        output_dir.join("summary.md"),
        "Fixed and committed the remaining medium finding. Did not push; working tree clean.",
    )
    .expect("write success-looking summary");
    fs::write(
        session_dir.join(csa_session::GATE_FAILURE_LOG_REL_PATH),
        "running just pre-commit\nFAIL [   0.005s] cli_sub_agent::gate::fails\nsecret=topsecret\nerror: Recipe `test` failed on line 42 with exit code 100\n",
    )
    .expect("write gate failure log");
    csa_session::save_result(
        &project_root,
        &session.meta_session_id,
        &failed_runtime_result(
            "Fixed and committed the remaining medium finding. Did not push; working tree clean.",
        ),
    )
    .expect("save failure envelope");

    let refreshed = refresh_and_repair_result(&project_root, &session.meta_session_id)
        .expect("refresh result")
        .expect("result should exist");

    assert_eq!(refreshed.status, "failure");
    assert_eq!(refreshed.exit_code, 1);
    assert!(
        refreshed.summary.starts_with("POST-EXEC GATE FAILED"),
        "summary must lead with gate failure, got: {}",
        refreshed.summary
    );
    assert!(
        !refreshed.summary.contains("working tree clean"),
        "success-looking child summary must not remain authoritative: {}",
        refreshed.summary
    );
    let report = refreshed
        .post_exec_gate
        .expect("post_exec_gate should be inferred from gate-failure.log");
    assert_eq!(report.gate_command, "post-exec gate");
    assert_eq!(report.exit_code, 1);
    assert_eq!(report.failing_step.as_deref(), Some("just test"));
    assert!(
        report
            .failing_tests
            .iter()
            .any(|test| test == "cli_sub_agent::gate::fails")
    );
    assert_eq!(report.log_path, csa_session::GATE_FAILURE_LOG_REL_PATH);
    assert!(
        !report.output_tail.contains("topsecret"),
        "inferred report must use redacted log content"
    );

    let loaded = csa_session::load_result(&project_root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert!(
        loaded.post_exec_gate.is_some(),
        "repair must persist the inferred gate detail for session result/wait"
    );
}

#[test]
fn refresh_and_repair_result_ignores_stale_gate_failure_log_for_unrelated_failure() {
    let fixture = create_stale_gate_log_failure_fixture();

    let refreshed = refresh_and_repair_result(&fixture.project_root, &fixture.session_id)
        .expect("refresh result")
        .expect("result should exist");

    assert_unrelated_signal_failure_preserved(&refreshed);
    assert_gate_log_is_diagnostic_only(&refreshed);
}

#[test]
fn refresh_and_repair_result_from_dir_ignores_stale_gate_failure_log_for_unrelated_failure() {
    let fixture = create_stale_gate_log_failure_fixture();

    let refreshed = refresh_and_repair_result_from_dir(&fixture.session_dir)
        .expect("refresh result")
        .expect("result should exist");

    assert_unrelated_signal_failure_preserved(&refreshed);
}

#[test]
fn issue_2440_clean_pass_repair_does_not_override_summary_gate() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let _state_guard =
        crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", temp.path().join("state"));
    let project_root = temp.path().join("project");
    fs::create_dir_all(&project_root).expect("create project");
    let session = csa_session::create_session(
        &project_root,
        Some("issue 2440 clean pass repair regression"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id;
    let session_dir =
        csa_session::get_session_dir(&project_root, &session_id).expect("session dir");
    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir).expect("create output dir");

    let summary = "No blocking findings in `main...HEAD`.\nHigh-severity: 1 finding remains in `crates/cli-sub-agent/src/session_observability.rs`.";
    fs::write(output_dir.join("summary.md"), summary).expect("write summary");
    let now = chrono::Utc::now();
    csa_session::write_review_meta(
        &session_dir,
        &csa_session::ReviewSessionMeta {
            session_id: session_id.clone(),
            head_sha: "issue-2440-head".to_string(),
            decision: csa_core::types::ReviewDecision::Pass.as_str().to_string(),
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
            timestamp: now,
            diff_fingerprint: None,
            fix_convergence: None,
        },
    )
    .expect("write review meta");
    csa_session::write_review_verdict(
        &session_dir,
        &csa_session::ReviewVerdictArtifact::from_parts(
            session_id.clone(),
            csa_core::types::ReviewDecision::Pass,
            "CLEAN",
            &[],
            vec![],
        ),
    )
    .expect("write pass verdict artifact");
    csa_session::save_result(
        &project_root,
        &session_id,
        &SessionResult {
            status: SessionResult::status_from_exit_code(1),
            exit_code: 1,
            summary: summary.to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            ..Default::default()
        },
    )
    .expect("save failing result");

    let refreshed = refresh_and_repair_result_from_dir(&session_dir)
        .expect("refresh result")
        .expect("result should exist");

    assert_eq!(refreshed.exit_code, 1);
    assert_eq!(refreshed.status, SessionResult::status_from_exit_code(1));
    let persisted: SessionResult = toml::from_str(
        &fs::read_to_string(session_dir.join(csa_session::result::RESULT_FILE_NAME))
            .expect("read persisted result"),
    )
    .expect("parse persisted result");
    assert_eq!(persisted.exit_code, 1);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(1));
}

#[test]
fn enrich_result_from_session_dir_ignores_stale_gate_failure_log_for_unrelated_failure() {
    let fixture = create_stale_gate_log_failure_fixture();
    let mut result = csa_session::load_result(&fixture.project_root, &fixture.session_id)
        .expect("load result")
        .expect("result should exist");

    let changed = enrich_result_from_session_dir(
        &fixture.project_root,
        &fixture.session_id,
        &fixture.session_dir,
        &mut result,
    )
    .expect("enrich result");

    assert!(
        changed,
        "diagnostic artifact discovery should update result"
    );
    assert_unrelated_signal_failure_preserved(&result);
    assert_gate_log_is_diagnostic_only(&result);
}

#[test]
fn refresh_and_repair_result_does_not_overwrite_in_flight_turn_sidecar() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let _state_guard =
        crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", temp.path().join("state"));
    let project_root = temp.path().join("project");
    fs::create_dir_all(&project_root).expect("create project");
    let session = csa_session::create_session(
        &project_root,
        Some("refresh sidecar ownership regression"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_dir =
        csa_session::get_session_dir(&project_root, &session.meta_session_id).expect("session dir");
    let turn_one_artifact = csa_session::turn_contract_result_artifact_path(1);
    let turn_two_artifact = csa_session::turn_contract_result_artifact_path(2);
    let turn_one_path = session_dir.join(&turn_one_artifact);
    let turn_two_path = session_dir.join(&turn_two_artifact);

    fs::create_dir_all(turn_one_path.parent().expect("turn one parent"))
        .expect("create turn one parent");
    fs::write(
        &turn_one_path,
        r#"
[report]
what_was_done = "completed turn one report"
"#,
    )
    .expect("write turn one sidecar");
    csa_session::save_result(
        &project_root,
        &session.meta_session_id,
        &runtime_result("completed turn one", &turn_one_artifact),
    )
    .expect("save completed turn one envelope");

    fs::create_dir_all(turn_two_path.parent().expect("turn two parent"))
        .expect("create turn two parent");
    fs::write(
        &turn_two_path,
        r#"
[report]
what_was_done = "in-flight turn two report"
"#,
    )
    .expect("write in-flight turn two sidecar");
    let turn_two_before = fs::read_to_string(&turn_two_path).expect("read turn two before");

    let refreshed = refresh_and_repair_result(&project_root, &session.meta_session_id)
        .expect("refresh result")
        .expect("result should exist");

    assert_eq!(
        fs::read_to_string(&turn_two_path).expect("read turn two after"),
        turn_two_before,
        "observation refresh must not overwrite an in-flight turn result sidecar"
    );
    assert!(
        fs::read_to_string(&turn_one_path)
            .expect("read turn one after")
            .contains("completed turn one report"),
        "owned turn one sidecar should remain the manager report source"
    );
    assert!(
        refreshed
            .artifacts
            .iter()
            .any(|artifact| artifact.path == turn_one_artifact && !artifact.display_only),
        "completed turn one sidecar remains the owned manager artifact"
    );
    assert!(
        refreshed
            .artifacts
            .iter()
            .any(|artifact| artifact.path == turn_two_artifact && artifact.display_only),
        "in-flight turn two sidecar remains visible but display-only"
    );

    let loaded = csa_session::load_result(&project_root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert_eq!(
        loaded
            .manager_fields
            .report
            .as_ref()
            .and_then(|value| value.get("what_was_done")),
        Some(&toml::Value::String(
            "completed turn one report".to_string()
        )),
        "subsequent loads must ignore display-only in-flight sidecars for manager overlay"
    );
}
