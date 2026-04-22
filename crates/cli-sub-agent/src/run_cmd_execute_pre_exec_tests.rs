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

#[tokio::test]
async fn handle_run_persists_result_for_model_spec_tier_conflict() {
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
        Some("inspect the repository".to_string()),
        None,
        None,
        None,
        None,
        false,
        None,
        false,
        None,
        false,
        None,
        None,
        false,
        Some(project_dir.path().display().to_string()),
        Some("codex/openai/gpt-5.4/high".to_string()),
        None,
        None,
        false,
        false,
        false,
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
        Vec::new(),
        Vec::new(),
    )
    .await
    .expect_err("model-spec + tier conflict must return an error");

    assert!(
        crate::run_helpers::is_routing_conflict(&err),
        "routing conflict classification should be structured: {err:#}"
    );
    assert!(
        err.chain()
            .any(|cause| cause.to_string().contains("Conflicting routing flags")),
        "unexpected error chain: {err:#}"
    );
    assert!(
        err.chain().any(|cause| cause
            .to_string()
            .contains("--model-spec and --tier are mutually exclusive")),
        "tier/model-spec conflict hint should be present: {err:#}"
    );

    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert_eq!(sessions.len(), 1, "expected one failed run session");

    let result = csa_session::load_result(project_dir.path(), &sessions[0].meta_session_id)
        .unwrap()
        .expect("result.toml must be written for pre-exec routing conflicts");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("pre-exec:"));
    assert!(
        result
            .summary
            .contains("--model-spec and --tier are mutually exclusive")
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
        Some("inspect the repository".to_string()),
        None,
        None,
        None,
        None,
        false,
        None,
        false,
        None,
        false,
        None,
        None,
        false,
        Some(project_dir.path().display().to_string()),
        None,
        None,
        None,
        false,
        false,
        false,
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
        Vec::new(),
        Vec::new(),
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
