use super::*;
use crate::review_cmd::tests::{
    ScopedEnvVarRestore, project_config_with_enabled_tools, setup_git_repo,
};
use crate::test_env_lock::ScopedTestEnvVar;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::{GlobalConfig, ProjectProfile, TierStrategy, config::TierConfig};
use csa_core::types::ToolName;

fn config_with_review_tier(enabled_tools: &[&str], models: &[&str]) -> csa_config::ProjectConfig {
    let mut config = project_config_with_enabled_tools(enabled_tools);
    for (name, tool_config) in &mut config.tools {
        tool_config.memory_max_mb = (name != "codex").then_some(256);
    }
    if enabled_tools.contains(&"codex") {
        crate::review_cmd::tests::configure_codex_cli_review_test_tool(&mut config);
    }
    config.tiers.insert(
        "quality".to_string(),
        TierConfig {
            description: "quality".to_string(),
            models: models.iter().map(|model| (*model).to_string()).collect(),
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    config
}

#[test]
fn issue_2409_review_tier_filters_codex_o3_before_candidate_selection() {
    let _available_guard =
        ScopedTestEnvVar::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let config = config_with_review_tier(
        &["codex", "claude-code"],
        &[
            "opencode/openai/gpt-5/xhigh",
            "codex/openai/o3/medium",
            "claude-code/anthropic/claude-sonnet-4-20250514/none",
        ],
    );
    let global = GlobalConfig::default();

    let (tool, model_spec) = crate::review_cmd::resolve_review_tool(
        None,
        None,
        Some(&config),
        &global,
        Some("codex"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("quality"),
        false,
    )
    .expect("compatible claude-code fallback should resolve");

    assert_eq!(tool, ToolName::ClaudeCode);
    assert_eq!(
        model_spec.as_deref(),
        Some("claude-code/anthropic/claude-sonnet-4-20250514/none")
    );

    let model_catalog = csa_config::EffectiveModelCatalog::shipped().expect("shipped catalog");
    let candidates = review_ordered_tier_candidates(ReviewTierCandidateRequest {
        initial_tool: tool,
        initial_model_spec: model_spec.as_deref(),
        tier_name: Some("quality"),
        project_config: Some(&config),
        global_config: Some(&global),
        model_catalog: &model_catalog,
        tier_fallback_enabled: true,
        no_failover: false,
        tier_preference_order: &[],
    })
    .unwrap();
    assert!(
        candidates
            .iter()
            .all(|(_, spec)| spec.as_deref() != Some("codex/openai/o3/medium")),
        "incompatible Codex o3 tier entry must not become an execution candidate: {candidates:?}"
    );

    let chain = crate::tier_model_fallback::build_fallback_chain_for_result(
        Some(&config),
        Some("quality"),
        &[],
        Some("claude-code/anthropic/claude-sonnet-4-20250514/none"),
        &[],
    );
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].tool, "opencode");
    assert_eq!(chain[0].skip_reason, "disabled");
    assert_eq!(chain[1].tool, "codex");
    assert_eq!(
        chain[1].model_spec.as_deref(),
        Some("codex/openai/o3/medium")
    );
    assert_eq!(chain[1].skip_reason, "incompatible-model");
    assert!(!chain[1].quota_exhausted);
}

#[test]
fn issue_2409_review_tier_without_compatible_model_errors_before_provider_selection() {
    let _available_guard =
        ScopedTestEnvVar::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let config = config_with_review_tier(
        &["codex"],
        &["opencode/openai/gpt-5/xhigh", "codex/openai/o3/medium"],
    );
    let global = GlobalConfig::default();

    let err = crate::review_cmd::resolve_review_tool(
        None,
        None,
        Some(&config),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("quality"),
        false,
    )
    .expect_err("all-disabled/incompatible tier should fail before provider selection");
    let message = err.to_string();
    assert!(message.contains("none of its tools are currently available"));
    assert!(
        message.contains("opencode/openai/gpt-5/xhigh=disabled"),
        "{message}"
    );
    assert!(
        message.contains("codex/openai/o3/medium=incompatible-model"),
        "{message}"
    );
}

#[test]
fn issue_2409_force_override_still_filters_codex_o3_before_candidate_selection() {
    let _available_guard =
        ScopedTestEnvVar::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let config = config_with_review_tier(
        &["codex", "claude-code"],
        &[
            "codex/openai/o3/medium",
            "claude-code/anthropic/claude-sonnet-4-20250514/none",
        ],
    );
    let global = GlobalConfig::default();

    let err = crate::review_cmd::resolve_review_tool(
        Some(ToolName::Codex),
        None,
        Some(&config),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        true,
        Some("quality"),
        false,
    )
    .expect_err("force override must not promote incompatible Codex o3");
    let message = err.to_string();
    assert!(
        message.contains("--tool codex") && message.contains("not a candidate"),
        "{message}"
    );
}

#[test]
fn issue_2409_force_override_still_allows_disabled_compatible_tool() {
    let _available_guard =
        ScopedTestEnvVar::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let config = config_with_review_tier(
        &["claude-code"],
        &[
            "codex/openai/gpt-5.4/medium",
            "claude-code/anthropic/claude-sonnet-4-20250514/none",
        ],
    );
    let global = GlobalConfig::default();

    let (tool, model_spec) = crate::review_cmd::resolve_review_tool(
        Some(ToolName::Codex),
        None,
        Some(&config),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        true,
        Some("quality"),
        false,
    )
    .expect("force override may bypass only the tool enablement gate");

    assert_eq!(tool, ToolName::Codex);
    assert_eq!(model_spec.as_deref(), Some("codex/openai/gpt-5.4/medium"));
}

#[cfg(unix)]
#[tokio::test]
async fn issue_2409_execute_review_does_not_invoke_codex_o3_when_fallback_is_available() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    std::fs::write(project_dir.path().join(".claude.json"), "{}\n").unwrap();
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let codex_invocation_log = project_dir.path().join("codex-invocations.log");
    let codex_invocation_log_str = codex_invocation_log.display().to_string();
    let codex_stub = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"{codex_invocation_log_str}\"\nprintf 'codex o3 should not be invoked\\n' >&2\nexit 42\n"
    );
    for binary in ["codex", "codex-acp"] {
        std::fs::write(bin_dir.join(binary), &codex_stub).unwrap();
    }
    std::fs::write(
        bin_dir.join("claude"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'claude 1.0.0\\n'\n  exit 0\nfi\nprintf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS via compatible fallback' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'No blocking issues found.' '<!-- CSA:SECTION:details:END -->'\n",
    )
    .unwrap();
    for binary in ["codex", "codex-acp", "claude"] {
        let path = bin_dir.join(binary);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = config_with_review_tier(
        &["codex", "claude-code"],
        &[
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
            "codex/openai/o3/medium",
            "claude-code/anthropic/claude-sonnet-4-20250514/none",
        ],
    );
    let global = GlobalConfig::default();

    let result = execute_review(
        ToolName::ClaudeCode,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        Some("claude-code/anthropic/claude-sonnet-4-20250514/none".to_string()),
        Some("quality".to_string()),
        true,
        None,
        "review: issue-2409-compatible-fallback".to_string(),
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
    .expect("compatible fallback should run");

    assert_eq!(result.executed_tool, ToolName::ClaudeCode);
    assert!(result.forced_decision.is_none());
    assert!(
        !codex_invocation_log.exists(),
        "Codex o3 must be filtered before provider invocation"
    );

    let persisted = csa_session::load_result(project_dir.path(), &result.execution.meta_session_id)
        .unwrap()
        .expect("result.toml should exist");
    let fallback_chain = persisted
        .fallback_chain
        .as_ref()
        .expect("routing exclusions should be persisted");
    assert_eq!(fallback_chain.len(), 2);
    assert_eq!(fallback_chain[1].tool, "codex");
    assert_eq!(
        fallback_chain[1].model_spec.as_deref(),
        Some("codex/openai/o3/medium")
    );
    assert_eq!(fallback_chain[1].skip_reason, "incompatible-model");
}
