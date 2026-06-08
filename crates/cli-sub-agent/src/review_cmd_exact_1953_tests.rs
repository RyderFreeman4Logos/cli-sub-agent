#[test]
fn issue_1953_structured_pass_empty_findings_updates_meta_wait_and_check_verdict() {
    let _guard = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let project_dir = exact_test_setup_git_repo();
    let _state_home = crate::test_env_lock::ScopedEnvVarRestore::set(
        "XDG_STATE_HOME",
        project_dir.path().join("state"),
    );
    let branch = "fix-1953-pass";
    let head_sha = csa_session::detect_git_head(project_dir.path()).expect("detect HEAD");
    let (session_id, session_dir) = exact_test_create_review_session(
        project_dir.path(),
        branch,
        &head_sha,
        "review: issue-1953 final pass",
    );

    let final_pass = concat!(
        "Reviewer progress: prior blocking defect is resolved.\n",
        "<!-- CSA:SECTION:summary -->\n",
        "PASS\n",
        "<!-- CSA:SECTION:summary:END -->\n\n",
        "<!-- CSA:SECTION:details -->\n",
        "Scope reviewed: `range:main...HEAD`.\n\n",
        "No blocking findings found.\n\n",
        "Prior-round recheck:\n",
        "- `src/mcp/server.rs:3952` novelty merge/audit ordering is resolved.\n\n",
        "Open questions: none.\n",
        "<!-- CSA:SECTION:details:END -->\n\n",
        "```findings.toml\n",
        "findings = []\n",
        "```\n",
    );
    std::fs::write(
        session_dir.join("output").join("full.md"),
        exact_test_codex_agent_message(final_pass),
    )
    .expect("write transcript");
    csa_session::persist_structured_output(&session_dir, final_pass)
        .expect("persist final structured output");

    let mut meta = exact_test_make_review_meta(&session_id, ReviewDecision::Fail, "HAS_ISSUES");
    meta.head_sha = head_sha.clone();
    meta.scope = "range:main...HEAD".to_string();
    meta.fix_attempted = false;
    meta.fix_rounds = 0;
    meta.exit_code = 1;

    let persisted_exit_code =
        review_cmd::persist_review_sidecars_if_session_exists(project_dir.path(), &meta, Some(&session_id));

    assert_eq!(persisted_exit_code, Some(0));
    let artifact: ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json")).unwrap(),
    )
    .unwrap();
    let persisted_meta: ReviewSessionMeta =
        serde_json::from_str(&std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap())
            .unwrap();
    let findings: FindingsFile = toml::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("findings.toml")).unwrap(),
    )
    .unwrap();

    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|count| *count == 0));
    assert_eq!(persisted_meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(persisted_meta.verdict, "CLEAN");
    assert_eq!(persisted_meta.exit_code, 0);
    assert!(findings.findings.is_empty());

    let wait_summary = crate::session_cmds_daemon::render_wait_result_summary(
        &session_dir,
        &session_id,
        &exact_test_wait_result(0, "PASS"),
    );
    assert!(wait_summary.contains("Review verdict: PASS"));

    let found = review_cmd::check_review_verdict_for_target(
        project_dir.path(),
        branch,
        &head_sha,
        "range:main...HEAD",
        None,
        None,
    )
    .unwrap()
    .expect("check-verdict should accept structured PASS with empty findings");
    assert_eq!(found.session_id, session_id);
}
