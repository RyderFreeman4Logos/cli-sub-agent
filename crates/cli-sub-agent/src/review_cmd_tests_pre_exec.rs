use super::*;

#[tokio::test]
async fn handle_review_persists_result_for_prior_rounds_summary_parse_failure() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir);
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

    let err = handle_review(args, 0)
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
