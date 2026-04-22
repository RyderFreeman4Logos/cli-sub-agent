use super::resolve_tool_and_model;
use csa_config::{ProjectConfig, ProjectMeta, ResourcesConfig};
use csa_core::types::ToolName;
use std::collections::HashMap;

#[test]
fn resolve_tool_and_model_model_spec_preserves_explicit_model_override() {
    let cfg = ProjectConfig {
        schema_version: 1,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
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

    let (tool, model_spec, model) = resolve_tool_and_model(
        None,
        Some("codex/openai/gpt-5.4/medium"),
        Some("explicit-model"),
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        None,
        false,
        false,
    )
    .expect("resolver should preserve explicit --model alongside --model-spec");

    assert_eq!(tool, ToolName::Codex);
    assert_eq!(model_spec.as_deref(), Some("codex/openai/gpt-5.4/medium"));
    assert_eq!(model.as_deref(), Some("explicit-model"));
}
