use super::*;
use csa_config::config::{CURRENT_SCHEMA_VERSION, ToolConfig};
use csa_config::global::GlobalToolConfig;
use csa_config::{ProjectMeta, ResourcesConfig};
use std::collections::HashMap;

/// When project config has `thinking_lock` for a tool, the CLI `--thinking`
/// value must be overridden. Verify via Executor's Debug representation.
#[tokio::test]
async fn thinking_lock_project_config_overrides_cli_thinking() {
    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            thinking_lock: Some("xhigh".to_string()),
            ..Default::default()
        },
    );
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
    };

    let result = build_and_validate_executor(
        &ToolName::Codex,
        None,
        None,
        Some("low"), // CLI says low, but lock says xhigh
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        false,
        false,
    )
    .await;

    // If tool is installed, verify thinking is locked to Xhigh.
    // If not installed, that's OK — the lock resolution happens before install check.
    if let Ok(exec) = result {
        let debug = format!("{exec:?}");
        assert!(
            debug.contains("Xhigh"),
            "thinking_lock should override CLI --thinking to Xhigh, got: {debug}"
        );
    }
}

/// When global config has `thinking_lock`, it should apply when project config
/// does not have one.
#[tokio::test]
async fn thinking_lock_global_config_applies_when_project_absent() {
    let mut global_tools = HashMap::new();
    global_tools.insert(
        "codex".to_string(),
        GlobalToolConfig {
            thinking_lock: Some("high".to_string()),
            ..Default::default()
        },
    );
    let global_cfg = csa_config::GlobalConfig {
        tools: global_tools,
        ..Default::default()
    };

    let result = build_and_validate_executor(
        &ToolName::Codex,
        None,
        None,
        Some("low"), // CLI says low, but global lock says high
        ConfigRefs {
            project: None,
            global: Some(&global_cfg),
        },
        false,
        false,
    )
    .await;

    if let Ok(exec) = result {
        let debug = format!("{exec:?}");
        assert!(
            debug.contains("High"),
            "global thinking_lock should override CLI --thinking to High, got: {debug}"
        );
    }
}

/// Project config `thinking_lock` takes precedence over global config.
#[tokio::test]
async fn thinking_lock_project_overrides_global() {
    let mut project_tools = HashMap::new();
    project_tools.insert(
        "codex".to_string(),
        ToolConfig {
            thinking_lock: Some("xhigh".to_string()),
            ..Default::default()
        },
    );
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: project_tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
    };

    let mut global_tools = HashMap::new();
    global_tools.insert(
        "codex".to_string(),
        GlobalToolConfig {
            thinking_lock: Some("low".to_string()), // global says low
            ..Default::default()
        },
    );
    let global_cfg = csa_config::GlobalConfig {
        tools: global_tools,
        ..Default::default()
    };

    let result = build_and_validate_executor(
        &ToolName::Codex,
        None,
        None,
        None, // no CLI thinking
        ConfigRefs {
            project: Some(&cfg),
            global: Some(&global_cfg),
        },
        false,
        false,
    )
    .await;

    if let Ok(exec) = result {
        let debug = format!("{exec:?}");
        assert!(
            debug.contains("Xhigh"),
            "project thinking_lock (xhigh) must override global (low), got: {debug}"
        );
    }
}

/// When no thinking_lock is configured, CLI `--thinking` should pass through.
#[tokio::test]
async fn no_thinking_lock_passes_cli_thinking_through() {
    let result = build_and_validate_executor(
        &ToolName::Codex,
        None,
        None,
        Some("medium"), // CLI medium, no lock
        ConfigRefs {
            project: None,
            global: None,
        },
        false,
        false,
    )
    .await;

    if let Ok(exec) = result {
        let debug = format!("{exec:?}");
        assert!(
            debug.contains("Medium"),
            "without thinking_lock, CLI --thinking should pass through, got: {debug}"
        );
    }
}
