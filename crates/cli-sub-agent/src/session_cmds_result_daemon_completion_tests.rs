use super::*;

#[cfg(unix)]
#[test]
fn handle_session_result_fails_closed_on_success_completion_without_result() {
    let tmp = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = tmp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", tmp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = tmp.path();

    let session = create_session(
        project,
        Some("result-success-completion-without-result"),
        None,
        Some("codex"),
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    backdate_tree(&session_dir, 120);
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .unwrap();

    handle_session_result(
        session_id.clone(),
        false,
        Some(project.to_string_lossy().into_owned()),
        StructuredOutputOpts::default(),
    )
    .unwrap();

    let result = load_result(project, &session_id)
        .unwrap()
        .expect("session result should synthesize fail-closed result");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.raw_process_exit_code, Some(0));
    assert!(
        result
            .summary
            .contains("treating daemon completion as failure"),
        "summary should explain fail-closed conversion: {}",
        result.summary
    );

    let persisted = csa_session::load_session(project, &session_id).unwrap();
    assert_eq!(persisted.phase, csa_session::SessionPhase::Retired);
    assert_eq!(
        persisted.termination_reason.as_deref(),
        Some("daemon_completion_missing_result")
    );
}

#[cfg(unix)]
#[test]
fn handle_session_result_writes_review_no_result_diagnostic_artifacts() {
    let tmp = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = tmp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", tmp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = tmp.path();

    let session = create_session(project, Some("initializing daemon review"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    backdate_tree(&session_dir, 120);
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 1\nstatus = \"failure\"\n",
    )
    .unwrap();

    handle_session_result(
        session_id.clone(),
        false,
        Some(project.to_string_lossy().into_owned()),
        StructuredOutputOpts::default(),
    )
    .unwrap();

    let result = load_result(project, &session_id)
        .unwrap()
        .expect("session result should synthesize from daemon review completion");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(
        result
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/review-verdict.json"),
        "result.toml should list the review verdict diagnostic artifact"
    );

    let review_meta: csa_session::ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(review_meta.decision, "unavailable");
    assert_eq!(review_meta.tool, "unknown");
    assert_eq!(
        review_meta.failure_reason.as_deref(),
        Some(
            "review daemon completion recorded status=failure exit_code=1 before result.toml was written; no tool launch metadata was recorded"
        )
    );

    let verdict: csa_session::ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        verdict.decision,
        csa_core::types::ReviewDecision::Unavailable
    );
    assert_eq!(
        verdict.failure_reason.as_deref(),
        review_meta.failure_reason.as_deref()
    );
}
