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

#[cfg(unix)]
#[test]
fn review_daemon_no_result_diagnostic_canonicalizes_review_description_scopes() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    for (description, task_type, expected_scope) in [
        (
            "review: range:main...HEAD",
            Some("review"),
            "range:main...HEAD",
        ),
        (
            "review[1]: range:main...HEAD",
            Some("reviewer_sub_session"),
            "range:main...HEAD",
        ),
        ("code-review: range:main...HEAD", None, "range:main...HEAD"),
    ] {
        let session = create_session(project, Some(description), None, None).unwrap();
        let session_id = session.meta_session_id;
        if let Some(task_type) = task_type {
            let mut session_state = load_session(project, &session_id).unwrap();
            session_state.task_context = csa_session::TaskContext {
                task_type: Some(task_type.to_string()),
                tier_name: None,
            };
            save_session(&session_state).unwrap();
        }
        let session_dir = get_session_dir(project, &session_id).unwrap();

        seed_daemon_session_env(&session_id, Some(project.to_string_lossy().as_ref()));
        persist_daemon_completion_from_env(1);

        let result = load_result(project, &session_id)
            .unwrap()
            .unwrap_or_else(|| panic!("{description} should synthesize result.toml"));
        assert!(
            result
                .artifacts
                .iter()
                .any(|artifact| artifact.path == "output/review-verdict.json"),
            "{description} should advertise review verdict diagnostic artifact"
        );
        let review_meta: csa_session::ReviewSessionMeta = serde_json::from_str(
            &std::fs::read_to_string(session_dir.join("review_meta.json"))
                .unwrap_or_else(|_| panic!("{description} should write review_meta.json")),
        )
        .unwrap();
        assert_eq!(review_meta.decision, "unavailable", "{description}");
        assert_eq!(review_meta.scope, expected_scope, "{description}");
        assert!(
            session_dir
                .join("output")
                .join("review-verdict.json")
                .exists(),
            "{description} should write review-verdict.json"
        );
    }
}

#[cfg(unix)]
#[test]
fn daemon_completion_before_result_uses_existing_review_verdict_artifact() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let mut session = create_session(
        project,
        Some("review: range:main...HEAD"),
        None,
        Some("codex"),
    )
    .unwrap();
    session.git_head_at_creation = Some("abcdef1234567890".to_string());
    session.task_context = csa_session::TaskContext {
        task_type: Some("review".to_string()),
        tier_name: None,
    };
    save_session(&session).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    csa_session::write_review_verdict(
        &session_dir,
        &csa_session::ReviewVerdictArtifact::from_parts(
            session_id.clone(),
            csa_core::types::ReviewDecision::Pass,
            "CLEAN",
            &[],
            Vec::new(),
        ),
    )
    .unwrap();

    seed_daemon_session_env(&session_id, Some(project.to_string_lossy().as_ref()));
    persist_daemon_completion_from_env(1);

    let result = load_result(project, &session_id)
        .unwrap()
        .expect("daemon completion should synthesize a result from review artifacts");
    assert_eq!(result.status, "success");
    assert_eq!(result.exit_code, 0);
    assert!(
        !result
            .summary
            .contains("no tool launch metadata was recorded"),
        "existing review artifacts must not be converted into no-result diagnostics: {}",
        result.summary
    );
    assert!(
        result
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/review-verdict.json"),
        "recovered result should advertise the existing review verdict artifact"
    );

    let review_meta: csa_session::ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(review_meta.session_id, session_id);
    assert_eq!(review_meta.head_sha, "abcdef1234567890");
    assert_eq!(review_meta.scope, "range:main...HEAD");
    assert_eq!(review_meta.decision, "pass");
    assert_eq!(review_meta.verdict, "CLEAN");
    assert_eq!(review_meta.exit_code, 0);
    assert_eq!(review_meta.status_reason, None);
    assert_eq!(review_meta.primary_failure, None);

    let verdict: csa_session::ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(verdict.decision, csa_core::types::ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert_eq!(verdict.primary_failure, None);
}

#[cfg(unix)]
#[test]
fn daemon_completion_review_artifact_recovery_prefers_non_pass_verdict_over_stale_pass_meta() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let mut session = create_session(
        project,
        Some("review: range:main...HEAD"),
        None,
        Some("codex"),
    )
    .unwrap();
    session.git_head_at_creation = Some("abcdef1234567890".to_string());
    session.task_context = csa_session::TaskContext {
        task_type: Some("review".to_string()),
        tier_name: None,
    };
    save_session(&session).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    csa_session::write_review_meta(
        &session_dir,
        &csa_session::ReviewSessionMeta {
            session_id: session_id.clone(),
            head_sha: "abcdef1234567890".to_string(),
            decision: csa_core::types::ReviewDecision::Pass.as_str().to_string(),
            verdict: "CLEAN".to_string(),
            review_mode: Some("standard".to_string()),
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
            diff_fingerprint: Some("sha256:stale-pass-meta".to_string()),
            fix_convergence: None,
        },
    )
    .unwrap();
    csa_session::write_review_verdict(
        &session_dir,
        &csa_session::ReviewVerdictArtifact::from_parts(
            session_id.clone(),
            csa_core::types::ReviewDecision::Fail,
            "HAS_ISSUES",
            &[],
            Vec::new(),
        ),
    )
    .unwrap();

    seed_daemon_session_env(&session_id, Some(project.to_string_lossy().as_ref()));
    persist_daemon_completion_from_env(0);

    let result = load_result(project, &session_id)
        .unwrap()
        .expect("daemon completion should synthesize a result from review artifacts");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.raw_process_exit_code, Some(0));
    assert!(
        result.summary.contains("HAS_ISSUES (fail)"),
        "summary should reflect authoritative non-pass verdict: {}",
        result.summary
    );

    let recovered_meta: csa_session::ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        recovered_meta.decision,
        csa_core::types::ReviewDecision::Fail.as_str()
    );
    assert_eq!(recovered_meta.verdict, "HAS_ISSUES");
    assert_eq!(recovered_meta.exit_code, 1);
    assert_eq!(recovered_meta.head_sha, "abcdef1234567890");
    assert_eq!(recovered_meta.scope, "range:main...HEAD");
    assert_eq!(
        recovered_meta.diff_fingerprint.as_deref(),
        Some("sha256:stale-pass-meta")
    );
}

#[cfg(unix)]
#[test]
fn daemon_completion_reconcile_late_real_review_result_does_not_keep_unavailable_diagnostics() {
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
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 1\nstatus = \"failure\"\n",
    )
    .unwrap();
    let late_result = SessionResult {
        summary: "late real review result".to_string(),
        ..make_result("success", 0)
    };

    let reconciled = ensure_terminal_result_for_dead_active_session_with_before_write(
        project,
        &session_id,
        "session wait",
        |_| {
            save_result(project, &session_id, &late_result).expect("persist late real result");
        },
    )
    .unwrap();

    assert_eq!(
        reconciled,
        DeadActiveSessionReconciliation::LateResultRetired
    );
    let result = load_result(project, &session_id)
        .unwrap()
        .expect("late real result should remain visible");
    assert_eq!(result.status, "success");
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "late real review result");
    assert!(
        !result
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/review-verdict.json"),
        "late real result must not advertise stale unavailable review verdict diagnostics"
    );
    assert!(
        !session_dir.join("review_meta.json").exists(),
        "late real result must not inherit unavailable review_meta.json"
    );
    assert!(
        !session_dir
            .join("output")
            .join("review-verdict.json")
            .exists(),
        "late real result must not inherit unavailable review-verdict.json"
    );
}
