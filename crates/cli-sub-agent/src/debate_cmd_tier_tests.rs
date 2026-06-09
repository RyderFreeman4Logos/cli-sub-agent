use super::*;
use crate::debate_cmd_resolve::resolve_debate_tool;
use crate::test_env_lock::ScopedTestEnvVar;
use csa_config::{GlobalConfig, ProjectConfig, ReviewConfig, ToolConfig, ToolSelection};
use csa_core::types::ToolName;
use std::collections::HashMap;

fn assume_tier_tools_available() -> ScopedTestEnvVar {
    ScopedTestEnvVar::set(
        crate::run_helpers::TEST_SKIP_TOOL_AVAILABILITY_CHECK_ENV,
        "1",
    )
}

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
        resources: csa_config::ResourcesConfig {
            memory_max_mb: Some(1024),
            min_free_memory_mb: 1,
            ..Default::default()
        },
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

fn debate_config_with_single_tool_whitelist(
    tier_name: &str,
    models: Vec<&str>,
    enabled_tools: &[&str],
    whitelist_tool: &str,
) -> ProjectConfig {
    debate_config_with_whitelist(tier_name, models, enabled_tools, &[whitelist_tool])
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
        None,
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
        msg.contains("[tier_policy].allow_force_bypass"),
        "should mention global escape hatch: {msg}"
    );
    assert!(
        !msg.contains("--force-ignore-tier-setting"),
        "direct-tool guard should not recommend bypass flags as normal remediation: {msg}"
    );
}

#[test]
fn test_debate_allows_tier_flag() {
    let _tool_availability = assume_tier_tools_available();
    let global = GlobalConfig::default();
    let cfg = debate_config_with_tier(
        "quality",
        vec!["gemini-cli/google/gemini-3.1-pro-preview/xhigh"],
        &["gemini-cli"],
    );
    let result = resolve_debate_tool(
        None,
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
    let _tool_availability = assume_tier_tools_available();
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
        None,
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
fn test_debate_tool_plus_tier_warns_and_uses_tier_when_tool_missing_from_tier() {
    let _tool_availability = assume_tier_tools_available();
    let global = GlobalConfig::default();
    let cfg = debate_config_with_tier(
        "quality",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
    );
    let result = resolve_debate_tool(
        Some(ToolName::Codex),
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("quality"),
        false,
    );

    let (tool, mode, model_spec) = result.expect("missing preferred tool should not hard-fail");
    assert_eq!(tool, ToolName::GeminiCli);
    assert_eq!(mode, DebateMode::Heterogeneous);
    assert_eq!(
        model_spec.as_deref(),
        Some("gemini-cli/google/default/xhigh")
    );
}

#[test]
fn test_debate_tier_preference_mismatch_uses_full_tier() {
    let _tool_availability = assume_tier_tools_available();
    let global = GlobalConfig::default();
    let cfg = debate_config_with_whitelist(
        "quality",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
        &["codex"],
    );
    let result = resolve_debate_tool(
        None,
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("quality"),
        false,
    );

    let (tool, mode, model_spec) = result.expect("absent preferred tool should not hard-fail");
    assert_eq!(tool, ToolName::GeminiCli);
    assert_eq!(mode, DebateMode::Heterogeneous);
    assert_eq!(
        model_spec.as_deref(),
        Some("gemini-cli/google/default/xhigh")
    );
}

#[test]
fn debate_resolution_prefers_configured_tool_without_narrowing_fallbacks() {
    let _tool_availability = assume_tier_tools_available();
    let global = GlobalConfig::default();
    let cfg = debate_config_with_single_tool_whitelist(
        "quality",
        vec![
            "gemini-cli/google/default/xhigh",
            "codex/openai/gpt-5.4/high",
        ],
        &["gemini-cli", "codex"],
        "codex",
    );

    let result = resolve_debate_tool(
        None,
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("quality"),
        false,
    )
    .expect("narrowing should soft-fallback by default");

    assert_eq!(result.0, ToolName::Codex);
    assert_eq!(result.1, DebateMode::Heterogeneous);
    assert_eq!(result.2.as_deref(), Some("codex/openai/gpt-5.4/high"));
}

#[test]
fn debate_resolution_require_heterogeneous_passes_with_preference_plus_fallbacks() {
    let _tool_availability = assume_tier_tools_available();
    let mut global = GlobalConfig::default();
    global.debate.require_heterogeneous = true;
    let cfg = debate_config_with_single_tool_whitelist(
        "quality",
        vec![
            "gemini-cli/google/default/xhigh",
            "codex/openai/gpt-5.4/high",
        ],
        &["gemini-cli", "codex"],
        "codex",
    );

    let result = resolve_debate_tool(
        None,
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("quality"),
        false,
    )
    .expect("strict heterogeneity should pass because preferences keep fallback tools");

    assert_eq!(result.0, ToolName::Codex);
    assert_eq!(result.1, DebateMode::Heterogeneous);
    assert_eq!(result.2.as_deref(), Some("codex/openai/gpt-5.4/high"));
}

#[test]
fn debate_resolution_passes_when_panel_stays_heterogeneous() {
    let _tool_availability = assume_tier_tools_available();
    let global = GlobalConfig::default();
    let cfg = debate_config_with_tier(
        "quality",
        vec![
            "gemini-cli/google/default/xhigh",
            "codex/openai/gpt-5.4/high",
        ],
        &["gemini-cli", "codex"],
    );

    let result = resolve_debate_tool(
        None,
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        Some("quality"),
        false,
    )
    .expect("heterogeneous panel should resolve");

    assert_eq!(result.0, ToolName::GeminiCli);
    assert_eq!(result.1, DebateMode::Heterogeneous);
    assert_eq!(result.2.as_deref(), Some("gemini-cli/google/default/xhigh"));
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
        None,
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
        None,
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
        None,
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
    let _tool_availability = assume_tier_tools_available();
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
