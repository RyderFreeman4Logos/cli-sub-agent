use super::*;
use crate::review_cmd::tests::{project_config_with_enabled_tools, setup_git_repo};
use crate::session_tier_failover::TIER_FAILOVER_SUPERSEDED_STATUS;
use crate::test_env_lock::ScopedEnvVarRestore;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::{ProjectProfile, TierStrategy, config::TierConfig};
use std::collections::HashMap;

#[test]
fn review_readonly_prompt_detection_skips_fix_prompts() {
    assert!(review_prompt_is_readonly(
        "Use the csa-review skill. scope=uncommitted, mode=review-only"
    ));
    assert!(!review_prompt_is_readonly(
        "Fix round 1/3. Fix all issues found in the review."
    ));
}

#[test]
fn with_readonly_session_env_injects_flag() {
    let mut base = HashMap::new();
    base.insert("EXISTING".to_string(), "value".to_string());

    let env = with_readonly_session_env(Some(&base), true).expect("env map");

    assert_eq!(env.get("EXISTING").map(String::as_str), Some("value"));
    assert_eq!(
        env.get(CSA_READONLY_SESSION_ENV).map(String::as_str),
        Some("1")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn openai_compat_review_fails_closed_before_provider_without_repo_tools() {
    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let mut config = project_config_with_enabled_tools(&["openai-compat"]);
    let openai_compat = config
        .tools
        .get_mut("openai-compat")
        .expect("openai-compat test config");
    openai_compat.base_url = Some("not a valid URL".to_string());
    openai_compat.api_key = Some("test-key".to_string());
    openai_compat.default_model = Some("test-model".to_string());

    let outcome = execute_review(
        ToolName::OpenaiCompat,
        "Use the csa-review skill. scope=uncommitted, mode=review-only".to_string(),
        None,
        None,
        None,
        None,
        false,
        None,
        "review: openai-compat-readonly-tools".to_string(),
        project_dir.path(),
        Some(&config),
        &GlobalConfig::default(),
        None,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        false,
        false,
        false,
        false,
        false,
        &[],
        &[],
        Some(false),
    )
    .await
    .expect("missing repository tools must produce an unavailable review outcome");

    assert_eq!(outcome.forced_decision, Some(ReviewDecision::Unavailable));
    assert_eq!(
        outcome.status_reason.as_deref(),
        Some("openai_compat_readonly_repo_tools_unavailable")
    );
    assert_eq!(outcome.execution.execution.exit_code, 1);
    assert!(outcome.execution.execution.output.is_empty());
    for required_tool in ["Bash", "Read", "Grep", "Glob"] {
        assert!(
            outcome
                .execution
                .execution
                .stderr_output
                .contains(required_tool),
            "missing required tool {required_tool}: {}",
            outcome.execution.execution.stderr_output
        );
    }
    let session_dir =
        csa_session::get_session_dir(project_dir.path(), &outcome.execution.meta_session_id)
            .expect("review session dir");
    assert!(!session_dir.join("output/findings.toml").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn openai_compat_review_missing_repo_tools_fails_over_to_second_reviewer() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create fixture bin directory");
    let codex = bin_dir.join("codex");
    std::fs::write(
        &codex,
        "#!/bin/sh\nprintf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS after OpenAI-compat repository-tool failover' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'The deterministic second reviewer supplied the final verdict.' '<!-- CSA:SECTION:details:END -->'\n",
    )
    .expect("write deterministic reviewer fixture");
    let mut permissions = std::fs::metadata(&codex)
        .expect("stat deterministic reviewer fixture")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&codex, permissions).expect("make deterministic reviewer executable");

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let mut config = project_config_with_enabled_tools(&["openai-compat", "codex"]);
    let openai_compat = config
        .tools
        .get_mut("openai-compat")
        .expect("openai-compat test config");
    openai_compat.base_url = Some("not a valid URL".to_string());
    openai_compat.api_key = Some("test-key".to_string());
    openai_compat.default_model = Some("test-model".to_string());
    crate::review_cmd::tests::configure_codex_cli_review_test_tool(&mut config);
    config.tiers.insert(
        "quality".to_string(),
        TierConfig {
            description: "quality".to_string(),
            models: vec![
                "openai-compat/openai/test-model/high".to_string(),
                "codex/openai/gpt-5.4/high".to_string(),
            ],
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    let global = GlobalConfig::default();

    let result = execute_review(
        ToolName::OpenaiCompat,
        "Use the csa-review skill. scope=uncommitted, mode=review-only".to_string(),
        None,
        None,
        Some("openai-compat/openai/test-model/high".to_string()),
        Some("quality".to_string()),
        true,
        None,
        "review: openai-compat-readonly-tools-tier-fallback".to_string(),
        project_dir.path(),
        Some(&config),
        &global,
        None,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        false,
        false,
        false,
        false,
        false,
        &[],
        &[],
        Some(false),
    )
    .await
    .expect("missing repository tools should fail over to the next reviewer");

    assert_eq!(result.executed_tool, ToolName::Codex);
    assert_eq!(
        result.routed_to.as_deref(),
        Some("codex/openai/gpt-5.4/high")
    );
    assert!(result.forced_decision.is_none());
    assert!(result.status_reason.is_none());
    assert!(
        result
            .execution
            .execution
            .output
            .contains("PASS after OpenAI-compat repository-tool failover")
    );

    let persisted = csa_session::load_result(project_dir.path(), &result.execution.meta_session_id)
        .expect("load final reviewer result")
        .expect("final reviewer result should exist");
    assert_eq!(persisted.original_tool.as_deref(), Some("openai-compat"));
    assert_eq!(persisted.fallback_tool.as_deref(), Some("codex"));

    let sessions =
        csa_session::list_sessions(project_dir.path(), None).expect("list reviewer sessions");
    let openai_sessions: Vec<_> = sessions
        .iter()
        .filter(|session| {
            csa_session::load_metadata(project_dir.path(), &session.meta_session_id)
                .expect("load reviewer session metadata")
                .is_some_and(|metadata| metadata.tool == "openai-compat")
        })
        .collect();
    assert_eq!(openai_sessions.len(), 1);
    let failed_session = openai_sessions[0];
    assert_eq!(failed_session.phase, csa_session::SessionPhase::Retired);
    let failed_result =
        csa_session::load_result(project_dir.path(), &failed_session.meta_session_id)
            .expect("load failed reviewer result")
            .expect("failed reviewer result should exist");
    assert_eq!(failed_result.status, TIER_FAILOVER_SUPERSEDED_STATUS);
    assert!(
        failed_result
            .summary
            .contains("openai_compat_readonly_repo_tools_unavailable")
    );
}

#[path = "review_cmd_execute_failover_classification_tests.rs"]
mod failover_classification_tests;

#[path = "review_cmd_execute_tier_1958_tests.rs"]
mod tier_1958_tests;
