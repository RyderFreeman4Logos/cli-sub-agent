use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use std::path::Path;

fn write_review_project_config(project_root: &Path, config: &ProjectConfig) {
    let config_path = ProjectConfig::config_path(project_root);
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(config_path, toml::to_string_pretty(config).unwrap()).unwrap();
}

fn install_pattern(project_root: &Path, name: &str) {
    let skill_dir = project_root
        .join(".csa")
        .join("patterns")
        .join(name)
        .join("skills")
        .join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# test pattern\n").unwrap();
}

#[tokio::test]
async fn daemon_direct_tool_tier_rejection_persists_pre_exec_result_before_completion() {
    let project_dir = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let mut config = project_config_with_enabled_tools(&["gemini-cli", "codex"]);
    config.tiers.insert(
        "default".to_string(),
        csa_config::config::TierConfig {
            description: "Test tier".to_string(),
            models: vec!["gemini-cli/google/default/xhigh".to_string()],
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    write_review_project_config(project_dir.path(), &config);
    install_pattern(project_dir.path(), "csa-review");

    let daemon_session = csa_session::create_session_fresh(
        project_dir.path(),
        Some("initializing daemon review"),
        None,
        None,
    )
    .unwrap();
    let session_id = daemon_session.meta_session_id.clone();
    let session_dir = csa_session::get_session_dir(project_dir.path(), &session_id).unwrap();
    let _daemon_id = ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_ID", &session_id);
    let _daemon_dir = ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_DIR", &session_dir);
    let _daemon_project = ScopedEnvVarRestore::set("CSA_DAEMON_PROJECT_ROOT", project_dir.path());

    let cd = project_dir.path().display().to_string();
    let args = parse_review_args(&[
        "csa",
        "review",
        "--cd",
        &cd,
        "--files",
        "src/lib.rs",
        "--tool",
        "codex",
        "--daemon-child",
        "--session-id",
        &session_id,
    ]);

    let err = handle_review(args, 0, &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV)
        .await
        .expect_err("direct --tool tier rejection must fail");
    assert!(
        err.chain().any(|cause| cause
            .to_string()
            .contains("restricted when tiers are configured")),
        "unexpected error chain: {err:#}"
    );

    let result = csa_session::load_result(project_dir.path(), &session_id)
        .unwrap()
        .expect("daemon review pre-exec failure should write result.toml before completion");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.tool, "codex");
    assert!(result.summary.contains("Direct --tool is restricted"));
    assert!(!result.summary.contains("tool launch metadata"));
    assert!(
        result
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/review-verdict.json"),
        "pre-exec review result should advertise the unavailable review verdict artifact"
    );

    let meta: ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(meta.tool, "codex");
    assert_eq!(meta.decision, ReviewDecision::Unavailable.as_str());
    assert_eq!(
        meta.primary_failure.as_deref(),
        Some("direct_tool_tier_restricted")
    );
    let artifact: csa_session::ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(artifact.decision, ReviewDecision::Unavailable);
    assert_eq!(
        artifact.primary_failure.as_deref(),
        Some("direct_tool_tier_restricted")
    );

    crate::session_cmds_daemon::persist_daemon_completion_from_env(1);
    let result_after_completion = csa_session::load_result(project_dir.path(), &session_id)
        .unwrap()
        .expect("daemon completion must preserve pre-exec result");
    assert_eq!(result_after_completion.summary, result.summary);
    assert!(
        !result_after_completion
            .summary
            .contains("tool_launch_metadata_absent")
    );
}
