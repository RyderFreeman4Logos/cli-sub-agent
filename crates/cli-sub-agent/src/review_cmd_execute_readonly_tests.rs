use super::*;
use crate::review_cmd::tests::{project_config_with_enabled_tools, setup_git_repo};
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::ProjectProfile;
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

#[path = "review_cmd_execute_failover_classification_tests.rs"]
mod failover_classification_tests;

#[path = "review_cmd_execute_tier_1958_tests.rs"]
mod tier_1958_tests;
