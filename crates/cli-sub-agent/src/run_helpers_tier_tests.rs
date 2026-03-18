use csa_config::{ProjectConfig, ProjectMeta, ResourcesConfig, TierStrategy, ToolConfig};
use csa_core::types::ToolName;
use std::collections::HashMap;

// --- resolve_tool_and_model enablement guard tests ---

#[test]
fn resolve_tool_and_model_disabled_tool_explicit_errors() {
    use csa_config::{ProjectConfig, ToolConfig};
    use std::collections::HashMap;

    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: false,
            ..Default::default()
        },
    );

    let config = ProjectConfig {
        schema_version: 1,
        project: Default::default(),
        resources: Default::default(),
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
    };

    let result = super::resolve_tool_and_model(
        Some(ToolName::Codex),
        None,
        None,
        Some(&config),
        std::path::Path::new("/tmp"),
        true,  // force tier bypass
        false, // no override
        false, // needs_edit
        None,  // tier
        false, // force_ignore_tier_setting
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("disabled in user configuration"), "{msg}");
}

#[test]
fn resolve_tool_and_model_disabled_tool_with_override_succeeds() {
    use csa_config::{ProjectConfig, ToolConfig};
    use std::collections::HashMap;

    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: false,
            ..Default::default()
        },
    );

    let config = ProjectConfig {
        schema_version: 1,
        project: Default::default(),
        resources: Default::default(),
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
    };

    let result = super::resolve_tool_and_model(
        Some(ToolName::Codex),
        None,
        None,
        Some(&config),
        std::path::Path::new("/tmp"),
        true,  // force tier bypass
        true,  // override enabled
        false, // needs_edit
        None,  // tier
        false, // force_ignore_tier_setting
    );
    assert!(result.is_ok());
    let (tool, _, _) = result.unwrap();
    assert_eq!(tool, ToolName::Codex);
}

#[test]
fn resolve_tool_and_model_disabled_tool_model_spec_errors() {
    use csa_config::{ProjectConfig, ToolConfig};
    use std::collections::HashMap;

    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: false,
            ..Default::default()
        },
    );

    let config = ProjectConfig {
        schema_version: 1,
        project: Default::default(),
        resources: Default::default(),
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
    };

    let result = super::resolve_tool_and_model(
        None,
        Some("codex/openai/gpt-5.3-codex/high"),
        None,
        Some(&config),
        std::path::Path::new("/tmp"),
        true,
        false,
        false, // needs_edit
        None,  // tier
        false, // force_ignore_tier_setting
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("disabled in user configuration"), "{msg}");
}

// --- resolve_tool_from_tier tests ---

fn config_with_tier(tier_name: &str, models: Vec<&str>, enabled_tools: &[&str]) -> ProjectConfig {
    let mut tools = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        let name = tool.as_str();
        tools.insert(
            name.to_string(),
            ToolConfig {
                enabled: enabled_tools.contains(&name),
                ..Default::default()
            },
        );
    }
    let mut tiers = HashMap::new();
    tiers.insert(
        tier_name.to_string(),
        csa_config::config::TierConfig {
            description: "Test tier".to_string(),
            models: models.into_iter().map(String::from).collect(),
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    ProjectConfig {
        schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: chrono::Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        vcs: Default::default(),
    }
}

#[test]
fn resolve_tool_from_tier_returns_none_for_missing_tier() {
    let cfg = config_with_tier(
        "tier-1",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli"],
    );
    let result = super::resolve_tool_from_tier("nonexistent-tier", &cfg, None, None);
    assert!(result.is_none());
}

#[test]
fn resolve_tool_from_tier_returns_first_available_when_no_parent() {
    let cfg = config_with_tier(
        "test-tier",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli"],
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, None, None);
    assert!(result.is_some());
    let res = result.unwrap();
    assert_eq!(res.tool, ToolName::GeminiCli);
    assert_eq!(res.model_spec, "gemini-cli/google/default/xhigh");
}

#[test]
fn resolve_tool_from_tier_prefers_heterogeneous() {
    // Parent=claude-code(Anthropic), tier has gemini-cli+claude-code; prefer gemini for heterogeneity.
    let cfg = config_with_tier(
        "test-tier",
        vec![
            "claude-code/anthropic/default/xhigh",
            "gemini-cli/google/default/xhigh",
        ],
        &["claude-code", "gemini-cli"],
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, Some("claude-code"), None);
    assert!(result.is_some());
    let res = result.unwrap();
    assert_eq!(res.tool, ToolName::GeminiCli);
    assert_eq!(res.model_spec, "gemini-cli/google/default/xhigh");
}

#[test]
fn resolve_tool_from_tier_falls_back_to_same_family_when_no_heterogeneous() {
    // Parent is claude-code, tier only has claude-code models.
    // No heterogeneous option — should still return the first available.
    let cfg = config_with_tier(
        "test-tier",
        vec!["claude-code/anthropic/default/xhigh"],
        &["claude-code"],
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, Some("claude-code"), None);
    assert!(result.is_some());
    let res = result.unwrap();
    assert_eq!(res.tool, ToolName::ClaudeCode);
}

#[test]
fn resolve_tool_from_tier_skips_disabled_tools() {
    // gemini-cli is disabled, only claude-code is enabled.
    let cfg = config_with_tier(
        "test-tier",
        vec![
            "gemini-cli/google/default/xhigh",
            "claude-code/anthropic/default/xhigh",
        ],
        &["claude-code"],
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, None, None);
    assert!(result.is_some());
    let res = result.unwrap();
    assert_eq!(res.tool, ToolName::ClaudeCode);
}

#[test]
fn resolve_tool_from_tier_returns_none_when_all_disabled() {
    // All tools in tier are disabled.
    let cfg = config_with_tier(
        "test-tier",
        vec!["gemini-cli/google/default/xhigh"],
        &[], // no enabled tools
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, None, None);
    assert!(result.is_none());
}

// --- Phase 2: tier enforcement tests ---

/// When tiers are configured, direct --tool without --force-ignore-tier-setting is blocked.
#[test]
fn resolve_tool_and_model_blocks_direct_tool_when_tiers_configured() {
    let cfg = config_with_tier(
        "default",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli"],
    );
    let result = super::resolve_tool_and_model(
        Some(ToolName::GeminiCli),
        None,
        None,
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false, // force (tier whitelist bypass)
        false, // force_override_user_config
        false, // needs_edit
        None,  // tier
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

/// When tiers are configured, direct --model-spec without --force-ignore-tier-setting is blocked.
#[test]
fn resolve_tool_and_model_blocks_direct_model_spec_when_tiers_configured() {
    let cfg = config_with_tier(
        "default",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli"],
    );
    let result = super::resolve_tool_and_model(
        None,
        Some("gemini-cli/google/gemini-3.1-pro/high"),
        None,
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        None,
        false,
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("restricted when tiers are configured"),
        "unexpected error: {msg}"
    );
}

/// When tiers are configured, direct --model alone (no --tool, no --model-spec) is blocked.
#[test]
fn resolve_tool_and_model_blocks_direct_model_alone_when_tiers_configured() {
    let cfg = config_with_tier(
        "default",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli"],
    );
    let result = super::resolve_tool_and_model(
        None,                 // no --tool
        None,                 // no --model-spec
        Some("custom-model"), // --model provided
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        None,  // no --tier
        false, // no --force-ignore-tier-setting
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("restricted when tiers are configured"),
        "unexpected error: {msg}"
    );
}

/// --force-ignore-tier-setting bypasses the enforcement.
#[test]
fn resolve_tool_and_model_force_ignore_tier_allows_direct_tool() {
    let cfg = config_with_tier(
        "default",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
    );
    let result = super::resolve_tool_and_model(
        Some(ToolName::Codex),
        None,
        None,
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false, // force_override_user_config
        false,
        None,
        true, // force_ignore_tier_setting
    );
    assert!(result.is_ok(), "should bypass: {}", result.unwrap_err());
    let (tool, _, _) = result.unwrap();
    assert_eq!(tool, ToolName::Codex);
}

/// --force (force_override_user_config) also bypasses tier enforcement.
#[test]
fn resolve_tool_and_model_force_override_user_config_allows_direct_tool() {
    let cfg = config_with_tier(
        "default",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
    );
    let result = super::resolve_tool_and_model(
        Some(ToolName::Codex),
        None,
        None,
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        true, // force_override_user_config → bypasses tier enforcement
        false,
        None,
        false, // force_ignore_tier_setting is false, but override_user_config covers it
    );
    assert!(result.is_ok(), "should bypass: {}", result.unwrap_err());
    let (tool, _, _) = result.unwrap();
    assert_eq!(tool, ToolName::Codex);
}

/// When tiers HashMap is empty (no tiers configured), direct --tool works normally.
#[test]
fn resolve_tool_and_model_no_tiers_allows_direct_tool() {
    let cfg = ProjectConfig {
        schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: chrono::Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(), // empty — no tiers configured
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        vcs: Default::default(),
    };
    let result = super::resolve_tool_and_model(
        Some(ToolName::Codex),
        None,
        None,
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        None,
        false,
    );
    // Should succeed — no tiers means no enforcement
    assert!(
        result.is_ok(),
        "empty tiers should not block: {}",
        result.unwrap_err()
    );
}

/// --tier resolves tool from tier definition and returns it.
#[test]
fn resolve_tool_and_model_tier_flag_resolves_from_tier() {
    let cfg = config_with_tier(
        "quality",
        vec!["gemini-cli/google/gemini-3.1-pro-preview/xhigh"],
        &["gemini-cli"],
    );
    let result = super::resolve_tool_and_model(
        None,
        None,
        None,
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        Some("quality"), // --tier quality
        false,
    );
    assert!(
        result.is_ok(),
        "tier resolution failed: {}",
        result.unwrap_err()
    );
    let (tool, model_spec, _) = result.unwrap();
    assert_eq!(tool, ToolName::GeminiCli);
    assert_eq!(
        model_spec.as_deref(),
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh")
    );
}

/// --tier with --tool simultaneously: --tier takes precedence when tiers are configured
/// (direct tool is blocked), but both --tier and --tool together where --tier resolves first.
#[test]
fn resolve_tool_and_model_tier_with_tool_blocked_without_force() {
    let cfg = config_with_tier(
        "quality",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
    );
    // --tier quality --tool codex (without --force-ignore-tier-setting)
    // Since tiers are configured and tool is Some, the enforcement blocks it.
    // But --tier is also provided... The enforcement check uses tier.is_none(),
    // so when --tier IS provided, the direct-tool block does NOT trigger.
    let result = super::resolve_tool_and_model(
        Some(ToolName::Codex),
        None,
        None,
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        Some("quality"),
        false,
    );
    // --tier is present, so enforcement skips (tier.is_none() == false).
    // The --tier branch resolves tool from tier, ignoring the --tool arg.
    assert!(
        result.is_ok(),
        "tier+tool should resolve via tier: {}",
        result.unwrap_err()
    );
    let (tool, _, _) = result.unwrap();
    assert_eq!(tool, ToolName::GeminiCli, "tier should win over --tool");
}
