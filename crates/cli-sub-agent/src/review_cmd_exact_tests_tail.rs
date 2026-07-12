#[test]
fn final_iteration_high_finding_fails_all_verdict_consumers() {
    let _guard = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let project_dir = exact_test_setup_git_repo();
    let _state_home = crate::test_env_lock::ScopedEnvVarRestore::set(
        "XDG_STATE_HOME",
        project_dir.path().join("state"),
    );
    let branch = "fix-1764-fail";
    let head_sha = csa_session::detect_git_head(project_dir.path()).expect("detect HEAD");
    let (session_id, session_dir) = exact_test_create_review_session(
        project_dir.path(),
        branch,
        &head_sha,
        "review: issue-1764 final fail",
    );

    let final_fail = concat!(
        "<!-- CSA:SECTION:summary -->\n",
        "Verdict: FAIL\n",
        "<!-- CSA:SECTION:summary:END -->\n\n",
        "<!-- CSA:SECTION:details -->\n",
        "A blocking high finding remains.\n\n",
        "```findings.toml\n",
        "[[findings]]\n",
        "id = \"blocking-high\"\n",
        "severity = \"high\"\n",
        "description = \"blocking high finding\"\n",
        "\n",
        "[[findings.file_ranges]]\n",
        "path = \"src/lib.rs\"\n",
        "start = 1\n",
        "```\n",
        "<!-- CSA:SECTION:details:END -->\n",
    );
    std::fs::write(
        session_dir.join("output").join("full.md"),
        exact_test_codex_agent_message(final_fail),
    )
    .expect("write transcript");
    csa_session::persist_structured_output(&session_dir, final_fail)
        .expect("persist final structured output");

    let mut meta = exact_test_make_review_meta(&session_id, ReviewDecision::Pass, "CLEAN");
    meta.head_sha = head_sha.clone();
    meta.scope = "range:main...HEAD".to_string();
    meta.fix_attempted = false;
    meta.fix_rounds = 0;
    meta.review_iterations = 3;

    let persisted_exit_code = review_cmd::persist_review_sidecars_if_session_exists(
        project_dir.path(),
        &meta,
        Some(&session_id),
    );

    assert_eq!(persisted_exit_code, Some(1));
    let artifact: ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json")).unwrap(),
    )
    .unwrap();
    let persisted_meta: ReviewSessionMeta =
        serde_json::from_str(&std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap())
            .unwrap();
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&1));
    assert_eq!(persisted_meta.decision, ReviewDecision::Fail.as_str());
    assert_eq!(persisted_meta.verdict, "HAS_ISSUES");
    assert_eq!(persisted_meta.exit_code, 1);

    let wait_summary = crate::session_cmds_daemon::render_wait_result_summary(
        &session_dir,
        &session_id,
        &exact_test_wait_result(1, "blocking high finding remains"),
    );
    assert!(wait_summary.contains("Review verdict: FAIL"));
    assert!(!wait_summary.contains("Review verdict: PASS"));

    let found = review_cmd::check_review_verdict_for_target(
        project_dir.path(),
        branch,
        &head_sha,
        "range:main...HEAD",
        None,
        None,
    )
    .unwrap();
    assert!(found.is_none(), "check-verdict must reject final blocking findings");
}

#[test]
fn persist_review_sidecars_returns_fail_exit_for_has_issues_artifact() {
    let project_dir = exact_test_setup_git_repo();
    let _state_home = test_env_lock::ScopedTestEnvVar::set(
        "XDG_STATE_HOME",
        project_dir.path().join("state"),
    );
    let session_id = "01TESTVERDICTFAIL000000000";
    let session_dir = csa_session::get_session_dir(project_dir.path(), session_id)
        .expect("resolve session dir");
    std::fs::create_dir_all(session_dir.join("output")).expect("create session output dir");
    let mut meta = exact_test_make_review_meta(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    meta.status_reason = Some("test_blocking_verdict".to_string());

    let persisted_exit_code = review_cmd::persist_review_sidecars_if_session_exists(
        project_dir.path(),
        &meta,
        Some(session_id),
    );

    assert_eq!(persisted_exit_code, Some(1));
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&std::fs::read_to_string(&verdict_path).unwrap()).unwrap();
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
}

#[test]
fn issue_1716_sidecars_fail_closed_and_agree_when_final_reviewer_failed() {
    let project_dir = exact_test_setup_git_repo();
    let _state_home = test_env_lock::ScopedTestEnvVar::set(
        "XDG_STATE_HOME",
        project_dir.path().join("state"),
    );
    let session_id = "01TEST1716SIDECARAGREE000";
    let session_dir = csa_session::get_session_dir(project_dir.path(), session_id)
        .expect("resolve session dir");
    std::fs::create_dir_all(session_dir.join("output")).expect("create session output dir");
    std::fs::write(
        session_dir.join("output").join("full.md"),
        "I'll inspect the diff and then produce findings.\n",
    )
    .expect("write setup-only output");

    let mut meta = exact_test_make_review_meta(session_id, ReviewDecision::Pass, "CLEAN");
    meta.exit_code = 137;
    meta.primary_failure = Some("API key not found".to_string());

    let persisted_exit_code = review_cmd::persist_review_sidecars_if_session_exists(
        project_dir.path(),
        &meta,
        Some(session_id),
    );

    assert_eq!(persisted_exit_code, Some(1));
    assert!(
        !session_dir
            .join("output")
            .join(".findings.toml.synthetic")
            .exists(),
        "failed final reviewer must not get a synthetic empty-CLEAN marker"
    );

    let persisted_meta: ReviewSessionMeta =
        serde_json::from_str(&std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap())
            .unwrap();
    let artifact: ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json")).unwrap(),
    )
    .unwrap();

    assert_eq!(persisted_meta.decision, ReviewDecision::Unavailable.as_str());
    assert_eq!(persisted_meta.verdict, "UNAVAILABLE");
    assert_eq!(artifact.decision, ReviewDecision::Unavailable);
    assert_eq!(artifact.verdict_legacy, "UNAVAILABLE");
    assert_eq!(persisted_meta.primary_failure, artifact.primary_failure);
}

#[test]
fn issue_1593_clean_verdict_artifact_writes_gate_marker_despite_stale_fail_meta() {
    let project_dir = exact_test_setup_git_repo();
    exact_test_run_git(project_dir.path(), &["checkout", "-b", "fix-1593-test"]);
    let _state_home = test_env_lock::ScopedTestEnvVar::set(
        "XDG_STATE_HOME",
        project_dir.path().join("state"),
    );
    let session_id = "01TEST1593CLEANVERDICT0000";
    let session_dir = csa_session::get_session_dir(project_dir.path(), session_id)
        .expect("resolve session dir");
    std::fs::create_dir_all(session_dir.join("output")).expect("create session output dir");

    let review_text = concat!(
        "<!-- CSA:SECTION:summary -->\n",
        "PASS\n",
        "<!-- CSA:SECTION:summary:END -->\n\n",
        "<!-- CSA:SECTION:details -->\n",
        "No blocking findings.\n\n",
        "```findings.toml\n",
        "findings = []\n",
        "```\n",
        "<!-- CSA:SECTION:details:END -->\n",
    );
    let full_output = serde_json::to_string(&serde_json::json!({
        "type": "item.completed",
        "item": {
            "type": "agent_message",
            "text": review_text
        }
    }))
    .expect("serialize transcript line");
    std::fs::write(session_dir.join("output").join("full.md"), full_output)
        .expect("write full output");
    csa_session::persist_structured_output(&session_dir, review_text)
        .expect("persist structured output");

    let mut meta = exact_test_make_review_meta(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    meta.head_sha = csa_session::detect_git_head(project_dir.path()).expect("detect HEAD");
    meta.scope = "range:main...HEAD".to_string();

    let persisted_exit_code = review_cmd::persist_review_sidecars_if_session_exists(
        project_dir.path(),
        &meta,
        Some(session_id),
    );

    assert_eq!(persisted_exit_code, Some(0));
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&std::fs::read_to_string(&verdict_path).unwrap()).unwrap();
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "CLEAN");
    assert!(artifact.severity_counts.values().all(|count| *count == 0));

    let marker_path =
        crate::review_gate::marker_path(project_dir.path(), "fix-1593-test", &meta.head_sha);
    assert!(
        marker_path.exists(),
        "clean derived verdict should write the pre-push gate marker"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_marks_unavailable_when_all_tier_models_fail() {
    if which::which("bwrap").is_err() {
        eprintln!("skipping: bwrap not installed (CI gap, see #987)");
        return;
    }

    let project_dir = exact_test_setup_git_repo();
    let _sandbox = test_session_sandbox::ScopedSessionSandbox::new(&project_dir).await;
    std::fs::write(project_dir.path().join(".claude.json"), "{}\n").unwrap();
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    exact_test_write_executable(
        &bin_dir,
        "gemini",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\nprintf \"reason: 'QUOTA_EXHAUSTED'; monthly spending cap reached\\n\" >&2\nexit 1\n",
    );
    exact_test_write_executable(
        &bin_dir,
        "codex",
        "#!/bin/sh\nprintf 'HTTP 401 Invalid API key\\n' >&2\nexit 1\n",
    );
    // claude-code now defaults to CLI transport (#1115/#1117 workaround);
    // the binary to stub is `claude` (not `claude-code-acp`).
    exact_test_write_executable(
        &bin_dir,
        "claude",
        "#!/bin/sh\nprintf 'HTTP 403 Forbidden\\n' >&2\nexit 1\n",
    );

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = test_env_lock::ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = exact_test_config_with_review_tier(
        &["gemini-cli", "codex", "claude-code"],
        &[
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
            "codex/openai/gpt-5.4/high",
            "claude-code/anthropic/claude-sonnet/high",
        ],
    );
    let global = GlobalConfig::default();

    let result = review_cmd::execute_review_for_tests(
        ToolName::GeminiCli,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string()),
        Some("quality".to_string()),
        true,
        None,
        "review: tier-all-failed".to_string(),
        project_dir.path(),
        Some(&config),
        &global,
        review_routing::ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        false,
        false,
        false,
        false,
        false,
        &[],
        &[],
        Some(false), // error_marker_scan_override: force scan OFF for marker-bearing fixtures (#1745)
    )
    .await
    .expect("all-failed fallback should still return an outcome");

    assert_eq!(result.forced_decision, Some(ReviewDecision::Unavailable));
    let failure_reason = result.failure_reason.expect("failure_reason");
    assert!(
        failure_reason.contains("gemini-cli/google/gemini-3.1-pro-preview/xhigh=QUOTA_EXHAUSTED")
    );
    assert!(failure_reason.contains("codex/openai/gpt-5.4/high=HTTP 401"));
    assert!(failure_reason.contains("claude-code/anthropic/claude-sonnet/high=HTTP 403"));
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_falls_back_to_next_tier_model_and_persists_routing_metadata() {
    if which::which("bwrap").is_err() {
        eprintln!("skipping: bwrap not installed (CI gap, see #987)");
        return;
    }

    let project_dir = exact_test_setup_git_repo();
    let _sandbox = test_session_sandbox::ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    exact_test_write_executable(
        &bin_dir,
        "gemini",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\nprintf \"reason: 'QUOTA_EXHAUSTED'; monthly spending cap reached\\n\" >&2\nexit 1\n",
    );
    exact_test_write_executable(
        &bin_dir,
        "codex",
        "#!/bin/sh\nprintf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'No blocking issues found.' '<!-- CSA:SECTION:details:END -->'\n",
    );

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = test_env_lock::ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = exact_test_config_with_review_tier(
        &["gemini-cli", "codex"],
        &[
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
            "codex/openai/gpt-5.4/high",
        ],
    );
    let global = GlobalConfig::default();

    let result = review_cmd::execute_review_for_tests(
        ToolName::GeminiCli,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string()),
        Some("quality".to_string()),
        true,
        None,
        "review: tier-fallback-success".to_string(),
        project_dir.path(),
        Some(&config),
        &global,
        review_routing::ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        false,
        false,
        false,
        false,
        false,
        &[],
        &[],
        Some(false), // error_marker_scan_override: force scan OFF for marker-bearing fixtures (#1745)
    )
    .await
    .expect("tier fallback should succeed");

    assert_eq!(result.executed_tool, ToolName::Codex);
    assert_eq!(
        result.routed_to.as_deref(),
        Some("codex/openai/gpt-5.4/high")
    );
    // #1852: codex fallback SUCCEEDED, so the failed-over-from gemini quota
    // error is provenance (kept in routed_to/result.toml), not a terminal
    // primary_failure.
    assert!(
        result.primary_failure.is_none(),
        "successful fallback must not record the failed-over-from error as primary_failure"
    );

    let meta = ReviewSessionMeta {
        session_id: result.execution.meta_session_id.clone(),
        head_sha: String::new(),
        decision: ReviewDecision::Pass.as_str().to_string(),
        verdict: "CLEAN".to_string(),
        status_reason: None,
        routed_to: result.routed_to.clone(),
        primary_failure: result.primary_failure.clone(),
        failure_reason: result.failure_reason.clone(),
        tool: result.executed_tool.as_str().to_string(),
        scope: "uncommitted".to_string(),
        exit_code: 0,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        review_mode: None,
        fix_convergence: None,
    };
    let session_dir =
        csa_session::get_session_dir(project_dir.path(), &result.execution.meta_session_id)
            .unwrap();
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if verdict_path.exists() {
        std::fs::remove_file(&verdict_path).unwrap();
    }
    let persisted_exit_code = review_cmd::persist_review_sidecars_if_session_exists(
        project_dir.path(),
        &meta,
        result.persistable_session_id.as_deref(),
    );
    assert_eq!(persisted_exit_code, Some(0));
    let artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&std::fs::read_to_string(&verdict_path).unwrap()).unwrap();
    assert_eq!(artifact.routed_to, result.routed_to);
    assert_eq!(artifact.primary_failure, result.primary_failure);
}

#[test]
fn execute_review_fix_loop_skipped_on_unavailable() {
    assert!(review_cmd::should_run_fix_loop(true, ReviewDecision::Fail));
    assert!(!review_cmd::should_run_fix_loop(
        true,
        ReviewDecision::Unavailable
    ));
    assert!(!review_cmd::should_run_fix_loop(true, ReviewDecision::Pass));
    assert!(!review_cmd::should_run_fix_loop(true, ReviewDecision::Skip));
    assert!(!review_cmd::should_run_fix_loop(
        true,
        ReviewDecision::Uncertain
    ));
    assert!(!review_cmd::should_run_fix_loop(
        false,
        ReviewDecision::Fail
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_unavailable_does_not_persist_session_artifacts() {
    let project_dir = exact_test_setup_git_repo();
    let _sandbox = test_session_sandbox::ScopedSessionSandbox::new(&project_dir).await;
    let meta = ReviewSessionMeta {
        session_id: "unknown".to_string(),
        head_sha: String::new(),
        decision: ReviewDecision::Unavailable.as_str().to_string(),
        verdict: "UNAVAILABLE".to_string(),
        status_reason: Some("tier_models_unavailable".to_string()),
        routed_to: None,
        primary_failure: Some("QUOTA_EXHAUSTED".to_string()),
        failure_reason: Some("quality exhausted".to_string()),
        tool: ToolName::GeminiCli.as_str().to_string(),
        scope: "uncommitted".to_string(),
        exit_code: 1,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 0,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        review_mode: None,
        fix_convergence: None,
    };
    let persisted_exit_code =
        review_cmd::persist_review_sidecars_if_session_exists(project_dir.path(), &meta, None);
    assert_eq!(persisted_exit_code, None);

    let unknown_output = csa_session::get_session_root(project_dir.path())
        .unwrap()
        .join("sessions")
        .join("unknown")
        .join("output");
    assert!(
        !unknown_output.exists(),
        "unexpected session sidecars leaked into {}",
        unknown_output.display()
    );
}

include!("review_cmd_exact_1953_tests.rs");
