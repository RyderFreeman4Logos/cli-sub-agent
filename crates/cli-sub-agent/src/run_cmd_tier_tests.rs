use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_core::types::{OutputFormat, ToolArg};
use std::collections::HashMap;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn test_cli_hint_difficulty_conflicts_with_tier() {
    let result = try_parse_cli(&[
        "csa",
        "run",
        "--hint-difficulty",
        "quick_question",
        "--tier",
        "tier-1-quick",
        "prompt",
    ]);
    assert!(result.is_err(), "hint-difficulty and tier should conflict");
}

#[test]
fn test_cli_hint_difficulty_conflicts_with_auto_route() {
    let result = try_parse_cli(&[
        "csa",
        "run",
        "--hint-difficulty",
        "quick_question",
        "--auto-route",
        "code",
        "prompt",
    ]);
    assert!(
        result.is_err(),
        "hint-difficulty and auto-route should conflict"
    );
}

fn run_config_with_tier(
    tier_name: &str,
    models: Vec<&str>,
    enabled_tools: &[&str],
) -> csa_config::ProjectConfig {
    let mut tool_map = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        let name = tool.as_str();
        tool_map.insert(
            name.to_string(),
            csa_config::ToolConfig {
                enabled: enabled_tools.contains(&name),
                ..Default::default()
            },
        );
    }

    let mut config = csa_config::ProjectConfig {
        schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
        project: csa_config::ProjectMeta {
            name: "test".to_string(),
            created_at: chrono::Utc::now(),
            max_recursion_depth: 5,
        },
        resources: csa_config::ResourcesConfig::default(),
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
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    config
}

fn write_project_config(project_root: &Path, config: &csa_config::ProjectConfig) {
    let config_path = csa_config::ProjectConfig::config_path(project_root);
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(config_path, toml::to_string_pretty(config).unwrap()).unwrap();
}

#[tokio::test]
async fn handle_run_persists_result_for_direct_tool_tier_rejection() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let config = run_config_with_tier(
        "default",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
    );
    write_project_config(project_dir.path(), &config);

    let err = handle_run(
        Some(ToolArg::Specific(ToolName::Codex)),
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
    .expect_err("direct --tool tier rejection must return an error");

    assert!(
        err.chain().any(|cause| cause
            .to_string()
            .contains("Direct --tool is blocked when tiers are configured")),
        "unexpected error chain: {err:#}"
    );
    assert!(
        err.chain()
            .any(|cause| cause.to_string().contains("--auto-route <intent>")),
        "auto-route hint should be present: {err:#}"
    );

    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert_eq!(sessions.len(), 1, "expected one failed run session");

    let result = csa_session::load_result(project_dir.path(), &sessions[0].meta_session_id)
        .unwrap()
        .expect("result.toml must be written for pre-exec tier rejection");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("pre-exec:"));
    assert!(result.summary.contains("Direct --tool is blocked"));
}
