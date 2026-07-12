use super::*;

#[tokio::test]
async fn handle_review_rejects_missing_prompt_file_before_pre_exec() {
    let project_dir = tempdir().unwrap();
    let cd = project_dir.path().display().to_string();
    let missing = project_dir.path().join("missing-review-prompt.md");
    let args = parse_review_args(&[
        "csa",
        "review",
        "--cd",
        &cd,
        "--diff",
        "--prompt-file",
        missing.to_str().expect("utf-8 path"),
    ]);

    let err = handle_review(args, 0, &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV)
        .await
        .expect_err("missing --prompt-file must fail before review execution");

    assert!(
        err.chain()
            .any(|cause| cause.to_string().contains("--prompt-file: failed to read")),
        "unexpected error chain: {err:#}"
    );
}

#[tokio::test]
async fn handle_review_persists_result_for_prior_rounds_summary_parse_failure() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    write_review_project_config(
        project_dir.path(),
        &project_config_with_enabled_tools(&["codex"]),
    );
    install_pattern(project_dir.path(), "csa-review");

    let seeded = csa_session::create_session(
        project_dir.path(),
        Some("daemon-review"),
        None,
        Some("codex"),
    )
    .expect("seed daemon review session");
    let seeded_session_id = seeded.meta_session_id;
    let cd = project_dir.path().display().to_string();
    let missing = project_dir.path().join("missing-prior-rounds.toml");
    let args = parse_review_args(&[
        "csa",
        "review",
        "--cd",
        &cd,
        "--files",
        "src/lib.rs",
        "--session-id",
        &seeded_session_id,
        "--prior-rounds-summary",
        missing.to_str().expect("utf-8 path"),
    ]);

    let err = handle_review(args, 0, &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV)
        .await
        .expect_err("missing prior-rounds summary must fail");
    assert!(
        err.chain().any(|cause| cause
            .to_string()
            .contains("Failed to read prior-rounds summary file")),
        "unexpected error chain: {err:#}"
    );

    let result = csa_session::load_result(project_dir.path(), &seeded_session_id)
        .unwrap()
        .expect("result.toml must be written for prior-rounds parse failure");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("pre-exec:"));
    assert!(
        result
            .summary
            .contains("Failed to read prior-rounds summary file")
    );
}

#[test]
fn review_verdict_carries_no_provider_launch_diagnostic() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let session = csa_session::create_session(
        project_dir.path(),
        Some("reviewer soft-limit admission"),
        None,
        Some("codex"),
    )
    .expect("create review session");
    let session_id = session.meta_session_id;
    let diagnostic = csa_session::NoProviderLaunchDiagnostic {
        schema_version: csa_session::NO_PROVIDER_LAUNCH_SCHEMA_VERSION,
        session_id: session_id.clone(),
        timestamp: chrono::Utc::now(),
        tool: "codex".to_string(),
        role: "reviewer".to_string(),
        session_class: Some("reviewer_sub_session".to_string()),
        denial_class: crate::resource_admission_soft_limit::MEMORY_SOFT_LIMIT_ADMISSION_REASON
            .to_string(),
        no_provider_launch: true,
        provider_side_effects: false,
        head_sha: None,
        scope: None,
        range: None,
        memory: csa_session::NoProviderLaunchMemoryDiagnostic {
            effective_memory_max_mb: Some(8192),
            soft_limit_percent: Some(70),
            soft_threshold_mb: Some(5734),
            required_floor_mb: Some(8192),
            required_memory_max_mb: Some(11_703),
            ..Default::default()
        },
        guidance: vec![
            "remove a lower memory override so Codex can use its 16384MB default".to_string(),
        ],
    };
    csa_session::save_result(
        project_dir.path(),
        &session_id,
        &csa_session::SessionResult {
            status: "failure".to_string(),
            exit_code: 1,
            summary: "pre-exec: CSA: memory_soft_limit_admission denied".to_string(),
            tool: "codex".to_string(),
            started_at: chrono::Utc::now(),
            completed_at: chrono::Utc::now(),
            artifacts: vec![csa_session::SessionArtifact::new(
                csa_session::NO_PROVIDER_LAUNCH_ARTIFACT_PATH,
            )],
            ..Default::default()
        },
    )
    .expect("save synthetic result");
    let session_dir = csa_session::get_session_dir(project_dir.path(), &session_id).unwrap();
    csa_session::write_no_provider_launch_diagnostic(&session_dir, &diagnostic)
        .expect("write synthetic no-provider sidecar");

    let review_meta = ReviewSessionMeta {
        session_id: session_id.clone(),
        head_sha: "abc123def456".to_string(),
        decision: ReviewDecision::Unavailable.as_str().to_string(),
        verdict: "UNAVAILABLE".to_string(),
        review_mode: Some("standard".to_string()),
        status_reason: Some(
            crate::resource_admission_soft_limit::MEMORY_SOFT_LIMIT_ADMISSION_REASON.to_string(),
        ),
        routed_to: None,
        primary_failure: Some(
            crate::resource_admission_soft_limit::MEMORY_SOFT_LIMIT_ADMISSION_REASON.to_string(),
        ),
        failure_reason: Some("reviewer provider did not launch".to_string()),
        tool: "codex".to_string(),
        scope: "range:main...HEAD".to_string(),
        exit_code: 1,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        fix_convergence: None,
    };

    persist_review_sidecars_if_session_exists(project_dir.path(), &review_meta, Some(&session_id));

    let verdict: csa_session::ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse");
    assert_eq!(verdict.decision, ReviewDecision::Unavailable);
    let no_provider = verdict
        .no_provider_launch
        .as_ref()
        .expect("review verdict should carry no-provider diagnostic");
    assert_eq!(no_provider.role, "reviewer");
    assert_eq!(no_provider.scope.as_deref(), Some("range:main...HEAD"));
    assert_eq!(no_provider.range.as_deref(), Some("main...HEAD"));
    assert_eq!(no_provider.head_sha.as_deref(), Some("abc123def456"));
    assert_eq!(no_provider.memory.required_memory_max_mb, Some(11_703));

    let sidecar: csa_session::NoProviderLaunchDiagnostic = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join(csa_session::NO_PROVIDER_LAUNCH_ARTIFACT_PATH))
            .expect("enriched no-verdict sidecar should exist"),
    )
    .expect("enriched no-verdict sidecar should parse");
    assert_eq!(sidecar.scope.as_deref(), Some("range:main...HEAD"));
}
