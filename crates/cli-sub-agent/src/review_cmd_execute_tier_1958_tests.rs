use super::super::*;
use crate::review_cmd::tests::{
    ScopedEnvVarRestore, project_config_with_enabled_tools, setup_git_repo,
};
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::{GlobalConfig, ProjectProfile, TierStrategy, config::TierConfig};
use csa_core::types::ToolName;

fn config_with_review_tier(enabled_tools: &[&str], models: &[&str]) -> csa_config::ProjectConfig {
    let mut config = project_config_with_enabled_tools(enabled_tools);
    if enabled_tools.contains(&"codex") {
        config.tools.get_mut("codex").unwrap().transport = Some(csa_config::TransportKind::Cli);
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

#[cfg(unix)]
#[tokio::test]
async fn execute_review_falls_back_from_gemini_status_400_to_codex() {
    use std::os::unix::fs::PermissionsExt;

    if which::which("bwrap").is_err() {
        eprintln!("skipping: bwrap not installed (CI gap, see #987)");
        return;
    }

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    std::fs::write(
        bin_dir.join("gemini"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\nprintf 'status: 400\\n' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::write(
        bin_dir.join("codex"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'codex-cli 1.0.0\\n'\n  exit 0\nfi\nprintf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'No blocking issues found.' '<!-- CSA:SECTION:details:END -->'\n",
    )
    .unwrap();
    for binary in ["gemini", "codex"] {
        let path = bin_dir.join(binary);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = config_with_review_tier(
        &["gemini-cli", "codex"],
        &[
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
            "codex/openai/gpt-5.4/high",
        ],
    );
    let global = GlobalConfig::default();

    let result = execute_review(
        ToolName::GeminiCli,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string()),
        Some("quality".to_string()),
        true,
        None,
        "review: gemini-400-tier-fallback-success".to_string(),
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
    .expect("Gemini status:400 tier fallback should reach Codex (#1958)");

    assert_eq!(result.executed_tool, ToolName::Codex);
    assert_eq!(
        result.routed_to.as_deref(),
        Some("codex/openai/gpt-5.4/high")
    );
    assert!(result.forced_decision.is_none());
    assert!(result.status_reason.is_none());
    assert!(result.primary_failure.is_none());
    assert!(result.failure_reason.is_none());

    let persisted = csa_session::load_result(project_dir.path(), &result.execution.meta_session_id)
        .unwrap()
        .expect("result.toml should exist");
    assert_eq!(persisted.original_tool.as_deref(), Some("gemini-cli"));
    assert_eq!(persisted.fallback_tool.as_deref(), Some("codex"));
    let fallback_chain = persisted
        .fallback_chain
        .as_ref()
        .expect("result fallback_chain");
    assert_eq!(fallback_chain.len(), 1);
    assert_eq!(fallback_chain[0].tool, "gemini-cli");
    assert_eq!(fallback_chain[0].skip_reason, "attempted-and-errored");
    assert!(!fallback_chain[0].quota_exhausted);
}
