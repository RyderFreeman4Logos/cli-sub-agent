use super::*;
use std::collections::HashMap;

use crate::{
    AcpConfig, ExecutionConfig, FilesystemSandboxConfig, HooksSection, MemoryConfig, ProjectMeta,
    ResourcesConfig, SessionConfig, ToolConfig, config::CURRENT_SCHEMA_VERSION,
    config_session::VcsConfig,
};

fn empty_config() -> ProjectConfig {
    ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: AcpConfig::default(),
        session: SessionConfig::default(),
        memory: MemoryConfig::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        hooks: HooksSection::default(),
        execution: ExecutionConfig::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: VcsConfig::default(),
        filesystem_sandbox: FilesystemSandboxConfig::default(),
    }
}

#[test]
fn tool_initial_response_timeout_reads_tool_override() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "codex".to_string(),
        ToolConfig {
            initial_response_timeout_seconds: Some(300),
            ..Default::default()
        },
    );

    assert_eq!(
        cfg.tool_initial_response_timeout_seconds("codex"),
        Some(300)
    );
    assert_eq!(
        cfg.tool_initial_response_timeout_seconds("claude-code"),
        None
    );
}

#[test]
fn tool_initial_response_timeout_preserves_explicit_zero() {
    let mut cfg = empty_config();
    cfg.tools.insert(
        "codex".to_string(),
        ToolConfig {
            initial_response_timeout_seconds: Some(0),
            ..Default::default()
        },
    );

    assert_eq!(cfg.tool_initial_response_timeout_seconds("codex"), Some(0));
}
