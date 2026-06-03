use super::handle_run;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::{ProjectConfig, ProjectMeta, ResourcesConfig, TierStrategy, ToolConfig};
use csa_core::types::OutputFormat;
use std::collections::HashMap;
use std::path::Path;
use tempfile::tempdir;

fn run_config_with_tier(
    tier_name: &str,
    models: Vec<&str>,
    enabled_tools: &[&str],
) -> ProjectConfig {
    let mut tool_map = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        let name = tool.as_str();
        tool_map.insert(
            name.to_string(),
            ToolConfig {
                enabled: enabled_tools.contains(&name),
                ..Default::default()
            },
        );
    }

    let mut config = ProjectConfig {
        schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: chrono::Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: tool_map,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };
    config.tiers.insert(
        tier_name.to_string(),
        csa_config::config::TierConfig {
            description: "Test tier".to_string(),
            models: models.into_iter().map(String::from).collect(),
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    config
}

fn write_project_config(project_root: &Path, config: &ProjectConfig) {
    let config_path = ProjectConfig::config_path(project_root);
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(config_path, toml::to_string_pretty(config).unwrap()).unwrap();
}

fn run_config_without_routable_tools() -> ProjectConfig {
    run_config_with_tier("default", Vec::new(), &[])
}

async fn run_preflight_fixture(project_root: &Path, no_preflight: bool) -> anyhow::Result<i32> {
    handle_run(
        None,
        None,
        None,
        None,
        Some("inspect the repository".to_string()),
        None,
        None,
        None,
        None,
        false,
        None,
        false,
        false,
        None,
        false,
        None,
        None,
        false,
        true,
        Some(project_root.display().to_string()),
        None,
        None,
        None,
        false,
        false,
        false,
        false,
        false,
        None,
        false,
        None,
        None,
        None,
        false,
        false,
        None,
        0,
        OutputFormat::Text,
        csa_process::StreamMode::BufferOnly,
        None,
        false,
        false,
        false, // no_error_marker_scan (#1745)
        no_preflight,
        false,
        false,
        Vec::new(),
        Vec::new(),
        crate::startup_env::StartupSubtreeEnv::default(),
    )
    .await
}

#[tokio::test]
async fn handle_run_rejects_model_spec_tier_bypass_before_session_creation() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let config = run_config_with_tier(
        "default",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
    );
    write_project_config(project_dir.path(), &config);

    let err = handle_run(
        None,
        None,
        None,
        None,
        Some("inspect the repository".to_string()),
        None,
        None,
        None,
        None,
        false,
        None,
        false,
        false,
        None,
        false,
        None,
        None,
        false,
        true,
        Some(project_dir.path().display().to_string()),
        Some("codex/openai/gpt-5.4/high".to_string()),
        None,
        None,
        false,
        false,
        false,
        false,
        false,
        None,
        false,
        None,
        None,
        None,
        false,
        false,
        None,
        0,
        OutputFormat::Text,
        csa_process::StreamMode::BufferOnly,
        Some("default".to_string()),
        false,
        false,
        false, // no_error_marker_scan (#1745)
        false,
        false,
        false,
        Vec::new(),
        Vec::new(),
        crate::startup_env::StartupSubtreeEnv::default(),
    )
    .await
    .expect_err("model-spec + tier conflict must return an error");

    assert!(
        err.chain()
            .any(|cause| cause.to_string().contains("Tier bypass is disabled")),
        "unexpected error chain: {err:#}"
    );
    assert!(
        err.chain().any(|cause| cause
            .to_string()
            .contains("[tier_policy].allow_force_bypass")),
        "tier bypass escape hatch hint should be present: {err:#}"
    );
    assert!(
        err.chain()
            .any(|cause| cause.to_string().contains("Refused flags: --model-spec")),
        "refused flag should be named: {err:#}"
    );

    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert!(
        sessions.is_empty(),
        "tier bypass gate should reject before session creation"
    );
}

#[tokio::test]
async fn handle_run_no_preflight_skips_ai_config_check() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let mut config = run_config_without_routable_tools();
    config.preflight.ai_config_symlink_check.enabled = true;
    write_project_config(project_dir.path(), &config);
    std::fs::write(project_dir.path().join("AGENTS.md"), "not a symlink").unwrap();

    let preflight_err = run_preflight_fixture(project_dir.path(), false)
        .await
        .expect_err("enabled preflight should reject regular AI config");
    assert!(
        preflight_err
            .to_string()
            .contains("preflight: AI-config symlink integrity check failed"),
        "unexpected error: {preflight_err:#}"
    );

    let routing_err = run_preflight_fixture(project_dir.path(), true)
        .await
        .expect_err("--no-preflight should skip to routing");
    assert!(
        routing_err
            .to_string()
            .contains("No tool specified and no tier-based or auto-selectable tool available"),
        "unexpected error: {routing_err:#}"
    );
}

#[tokio::test]
async fn handle_run_does_not_persist_result_for_non_conflict_pre_exec_error() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let config = run_config_without_routable_tools();
    write_project_config(project_dir.path(), &config);

    let err = handle_run(
        None,
        None,
        None,
        None,
        Some("inspect the repository".to_string()),
        None,
        None,
        None,
        None,
        false,
        None,
        false,
        false,
        None,
        false,
        None,
        None,
        false,
        true,
        Some(project_dir.path().display().to_string()),
        None,
        None,
        None,
        false,
        false,
        false,
        false,
        false,
        None,
        false,
        None,
        None,
        None,
        false,
        false,
        None,
        0,
        OutputFormat::Text,
        csa_process::StreamMode::BufferOnly,
        None,
        false,
        false,
        false, // no_error_marker_scan (#1745)
        false,
        false,
        false,
        Vec::new(),
        Vec::new(),
        crate::startup_env::StartupSubtreeEnv::default(),
    )
    .await
    .expect_err("non-conflict pre-exec error must return an error");

    assert!(
        !crate::run_helpers::is_routing_conflict(&err),
        "unrelated pre-exec failures should not classify as routing conflicts: {err:#}"
    );
    assert!(
        err.to_string()
            .contains("No tool specified and no tier-based or auto-selectable tool available"),
        "unexpected error: {err:#}"
    );

    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert!(
        sessions.is_empty(),
        "non-conflict pre-exec errors should not create persisted run sessions"
    );
}
