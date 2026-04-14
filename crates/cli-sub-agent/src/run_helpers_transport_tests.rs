use super::build_executor;
use csa_config::{ProjectConfig, ProjectMeta, ResourcesConfig, ToolConfig, ToolTransport};
use csa_core::types::ToolName;
use csa_executor::{CodexTransport, Executor};
use std::collections::HashMap;

fn project_config_with_codex_tool(tool_config: ToolConfig) -> ProjectConfig {
    let mut tools = HashMap::new();
    tools.insert("codex".to_string(), tool_config);

    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
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
        execution: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

#[test]
fn build_executor_codex_transport_defaults_to_build_setting_when_unset() {
    let config = project_config_with_codex_tool(ToolConfig::default());
    let exec = build_executor(&ToolName::Codex, None, None, None, Some(&config), true).unwrap();

    match exec {
        Executor::Codex {
            runtime_metadata, ..
        } => {
            assert_eq!(
                runtime_metadata.transport_mode(),
                CodexTransport::default_for_build()
            );
        }
        other => panic!("expected codex executor, got: {other:?}"),
    }
}

#[test]
fn build_executor_codex_transport_respects_explicit_cli_override() {
    let config = project_config_with_codex_tool(ToolConfig {
        transport: Some(ToolTransport::Cli),
        ..Default::default()
    });
    let exec = build_executor(&ToolName::Codex, None, None, None, Some(&config), true).unwrap();

    match exec {
        Executor::Codex {
            runtime_metadata, ..
        } => {
            assert_eq!(runtime_metadata.transport_mode(), CodexTransport::Cli);
        }
        other => panic!("expected codex executor, got: {other:?}"),
    }
}

#[test]
fn build_executor_codex_transport_respects_explicit_acp_override() {
    let config = project_config_with_codex_tool(ToolConfig {
        transport: Some(ToolTransport::Acp),
        ..Default::default()
    });
    let exec = build_executor(&ToolName::Codex, None, None, None, Some(&config), true).unwrap();

    match exec {
        Executor::Codex {
            runtime_metadata, ..
        } => {
            assert_eq!(runtime_metadata.transport_mode(), CodexTransport::Acp);
        }
        other => panic!("expected codex executor, got: {other:?}"),
    }
}
