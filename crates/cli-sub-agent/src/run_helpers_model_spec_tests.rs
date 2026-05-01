use super::resolve_tool_and_model;
use csa_config::{
    ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, TierStrategy, ToolConfig,
};
use csa_core::types::ToolName;
use std::collections::HashMap;

fn project_config_with_tier_tools(tools: &[&str]) -> ProjectConfig {
    let mut tool_map = HashMap::new();
    let mut tier_models = Vec::new();
    for tool in tools {
        tool_map.insert(
            (*tool).to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
        tier_models.push(format!("{tool}/provider/model/medium"));
    }

    let mut tiers = HashMap::new();
    if !tier_models.is_empty() {
        tiers.insert(
            "tier3".to_string(),
            TierConfig {
                description: "test".to_string(),
                models: tier_models,
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        );
    }

    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: tool_map,
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::from([("default".to_string(), "tier3".to_string())]),
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
    }
}

#[test]
fn resolve_tool_and_model_allows_matching_tool_with_model_spec_when_tiers_configured() {
    let config = project_config_with_tier_tools(&["codex", "gemini-cli"]);

    let (tool, model_spec, model) = resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        model_spec: Some("codex/openai/gpt-5.4/medium"),
        config: Some(&config),
        project_root: std::path::Path::new("/tmp/test-project"),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp/test-project"))
    })
    .expect("matching --tool + --model-spec should bypass tier enforcement");

    assert_eq!(tool, ToolName::Codex);
    assert_eq!(model_spec.as_deref(), Some("codex/openai/gpt-5.4/medium"));
    assert!(model.is_none());
}

#[test]
fn resolve_tool_and_model_rejects_mismatched_tool_and_model_spec() {
    let config = project_config_with_tier_tools(&["codex", "gemini-cli"]);

    let error = resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::GeminiCli),
        model_spec: Some("codex/openai/gpt-5.4/medium"),
        config: Some(&config),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp/test-project"))
    })
    .expect_err("mismatched --tool + --model-spec must error");

    let message = error.to_string();
    assert!(message.contains("--tool gemini-cli"));
    assert!(message.contains("--model-spec codex/openai/gpt-5.4/medium"));
    assert!(message.contains("tool codex"));
}

#[test]
fn resolve_tool_and_model_preserves_explicit_model_override_with_model_spec() {
    let (tool, model_spec, model) = resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        model_spec: Some("codex/openai/gpt-5.4/medium"),
        model: Some("override-model"),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp/test-project"))
    })
    .expect("resolver should preserve explicit model override");

    assert_eq!(tool, ToolName::Codex);
    assert_eq!(model_spec.as_deref(), Some("codex/openai/gpt-5.4/medium"));
    assert_eq!(model.as_deref(), Some("override-model"));
}
