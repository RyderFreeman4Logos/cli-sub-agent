use super::*;
use crate::debate_cmd_resolve::resolve_debate_tool;
use csa_config::{GlobalConfig, ProjectConfig, ReviewConfig, ToolConfig, ToolSelection};
use csa_core::types::ToolName;
use std::collections::HashMap;

fn project_config_with_enabled_tools(tools: &[&str]) -> ProjectConfig {
    let mut tool_map = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        let name = tool.as_str();
        tool_map.insert(
            name.to_string(),
            ToolConfig {
                enabled: tools.contains(&name),
                ..Default::default()
            },
        );
    }
    ProjectConfig {
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
        execution: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

fn debate_config_with_tier(
    tier_name: &str,
    models: Vec<&str>,
    enabled_tools: &[&str],
) -> ProjectConfig {
    let mut cfg = project_config_with_enabled_tools(enabled_tools);
    cfg.tiers.insert(
        tier_name.to_string(),
        csa_config::config::TierConfig {
            description: "Test tier".to_string(),
            models: models.into_iter().map(String::from).collect(),
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    cfg
}

fn debate_config_with_whitelist(
    tier_name: &str,
    models: Vec<&str>,
    enabled_tools: &[&str],
    whitelist: &[&str],
) -> ProjectConfig {
    let mut cfg = debate_config_with_tier(tier_name, models, enabled_tools);
    cfg.debate = Some(ReviewConfig {
        tool: ToolSelection::Whitelist(whitelist.iter().map(|tool| (*tool).to_string()).collect()),
        ..Default::default()
    });
    cfg
}

#[test]
fn test_debate_blocks_direct_tool_when_tiers_configured() {
    let global = GlobalConfig::default();
    let cfg = debate_config_with_tier(
        "default",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
    );
    let result = resolve_debate_tool(
        Some(ToolName::Codex),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("restricted when tiers are configured"),
        "unexpected error: {msg}"
    );
    assert!(
        msg.contains("--force-ignore-tier-setting"),
        "should mention override flag: {msg}"
    );
}

#[test]
fn test_debate_allows_tier_flag() {
    let global = GlobalConfig::default();
    let cfg = debate_config_with_tier(
        "quality",
        vec!["gemini-cli/google/gemini-3.1-pro-preview/xhigh"],
        &["gemini-cli"],
    );
    let result = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("quality"), // cli_tier
        false,           // force_ignore_tier_setting
    );
    assert!(
        result.is_ok(),
        "tier flag should resolve: {}",
        result.unwrap_err()
    );
    let (tool, mode, model_spec) = result.unwrap();
    assert_eq!(tool, ToolName::GeminiCli);
    assert_eq!(mode, DebateMode::Heterogeneous);
    assert_eq!(
        model_spec.as_deref(),
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh")
    );
}

#[test]
fn test_debate_tool_plus_tier_resolves_requested_tool_from_tier() {
    let global = GlobalConfig::default();
    let cfg = debate_config_with_tier(
        "quality",
        vec![
            "gemini-cli/google/default/xhigh",
            "codex/openai/gpt-5.4/high",
            "claude-code/anthropic/sonnet-4.6/xhigh",
        ],
        &["gemini-cli", "codex", "claude-code"],
    );
    let result = resolve_debate_tool(
        Some(ToolName::Codex),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("quality"),
        false,
    );

    assert!(
        result.is_ok(),
        "tool+tier should resolve requested debate tool: {}",
        result.unwrap_err()
    );
    let (tool, mode, model_spec) = result.unwrap();
    assert_eq!(tool, ToolName::Codex);
    assert_eq!(mode, DebateMode::Heterogeneous);
    assert_eq!(model_spec.as_deref(), Some("codex/openai/gpt-5.4/high"));
}

#[test]
fn test_debate_tool_plus_tier_errors_when_tool_missing_from_tier() {
    let global = GlobalConfig::default();
    let cfg = debate_config_with_tier(
        "quality",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
    );
    let result = resolve_debate_tool(
        Some(ToolName::Codex),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("quality"),
        false,
    );

    let err = result.expect_err("missing tool in debate tier must error");
    assert!(
        err.to_string()
            .contains("Tool 'codex' is not available in tier 'quality'"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_debate_tier_whitelist_mismatch_errors_instead_of_bypassing_tier() {
    let global = GlobalConfig::default();
    let cfg = debate_config_with_whitelist(
        "quality",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
        &["codex"],
    );
    let result = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("quality"),
        false,
    );

    let err = result.expect_err("whitelist mismatch must hard-fail");
    let msg = err.to_string();
    assert!(
        msg.contains("[debate].tool whitelist"),
        "unexpected error: {msg}"
    );
    assert!(
        msg.contains("active debate tier remains authoritative"),
        "{msg}"
    );
}

#[test]
fn test_debate_force_ignore_tier_allows_direct_tool() {
    let global = GlobalConfig::default();
    let cfg = debate_config_with_tier(
        "default",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
    );
    let result = resolve_debate_tool(
        Some(ToolName::Codex),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None, // cli_tier
        true, // force_ignore_tier_setting
    );
    assert!(
        result.is_ok(),
        "force_ignore_tier_setting should bypass: {}",
        result.unwrap_err()
    );
    let (tool, mode, _) = result.unwrap();
    assert_eq!(tool, ToolName::Codex);
    assert_eq!(mode, DebateMode::Heterogeneous);
}

#[test]
fn test_debate_tool_plus_tier_and_force_ignore_errors_on_conflict() {
    let global = GlobalConfig::default();
    let cfg = debate_config_with_tier(
        "quality",
        vec![
            "gemini-cli/google/default/xhigh",
            "codex/openai/gpt-5.4/xhigh",
        ],
        &["gemini-cli", "codex"],
    );
    let result = resolve_debate_tool(
        Some(ToolName::Codex),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("quality"),
        true,
    );

    let err = result.expect_err("tool+tier+force-ignore must reject contradictory routing");
    let msg = err.to_string();
    assert!(
        msg.contains("Conflicting routing flags"),
        "unexpected error: {msg}"
    );
    assert!(msg.contains("--tier"), "unexpected error: {msg}");
    assert!(
        msg.contains("--force-ignore-tier-setting"),
        "unexpected error: {msg}"
    );
}

#[test]
fn test_debate_no_tiers_allows_direct_tool() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["codex"]);
    // No tiers configured — direct --tool should work
    let result = resolve_debate_tool(
        Some(ToolName::Codex),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
    );
    assert!(
        result.is_ok(),
        "no tiers should not block: {}",
        result.unwrap_err()
    );
}

#[test]
fn test_debate_tier_alias_resolves() {
    let global = GlobalConfig::default();
    let mut cfg = debate_config_with_tier(
        "quality",
        vec!["gemini-cli/google/gemini-3.1-pro-preview/xhigh"],
        &["gemini-cli"],
    );
    cfg.tier_mapping
        .insert("security_audit".to_string(), "quality".to_string());

    let result = resolve_debate_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("security_audit"), // tier_mapping alias
        false,
    );
    assert!(
        result.is_ok(),
        "tier alias should resolve: {}",
        result.unwrap_err()
    );
    let (tool, mode, model_spec) = result.unwrap();
    assert_eq!(tool, ToolName::GeminiCli);
    assert_eq!(mode, DebateMode::Heterogeneous);
    assert_eq!(
        model_spec.as_deref(),
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh")
    );
}
