use super::*;

#[cfg(unix)]
#[test]
fn persist_daemon_completion_from_env_writes_review_no_result_diagnostic_artifacts() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(project, Some("initializing daemon review"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    seed_daemon_session_env(&session_id, Some(project.to_string_lossy().as_ref()));
    persist_daemon_completion_from_env(1);

    let result = load_result(project, &session_id)
        .unwrap()
        .expect("daemon review completion should synthesize result.toml");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(
        result
            .summary
            .contains("no tool launch metadata was recorded"),
        "summary should include explicit tool metadata absence: {}",
        result.summary
    );
    assert!(
        result
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/review-verdict.json"),
        "synthetic result should advertise the review verdict diagnostic artifact"
    );

    let review_meta: csa_session::ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(review_meta.decision, "unavailable");
    assert_eq!(review_meta.verdict, "UNAVAILABLE");
    assert_eq!(review_meta.tool, "unknown");
    assert_eq!(
        review_meta.status_reason.as_deref(),
        Some("daemon_completion_before_result")
    );
    assert_eq!(
        review_meta.primary_failure.as_deref(),
        Some("tool_launch_metadata_absent")
    );

    let verdict: csa_session::ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        verdict.decision,
        csa_core::types::ReviewDecision::Unavailable
    );
    assert_eq!(verdict.verdict_legacy, "UNAVAILABLE");
    assert_eq!(
        verdict.primary_failure.as_deref(),
        Some("tool_launch_metadata_absent")
    );
    assert!(
        verdict
            .failure_reason
            .as_deref()
            .unwrap_or_default()
            .contains("no tool launch metadata was recorded")
    );
}
