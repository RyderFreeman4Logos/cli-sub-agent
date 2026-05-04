use crate::test_env_lock::ScopedTestEnvVar;
use csa_config::{ProjectConfig, ProjectMeta, ResourcesConfig, TierStrategy, ToolConfig};
use csa_core::types::ToolName;
use std::collections::HashMap;

fn assume_tier_tools_available() -> ScopedTestEnvVar {
    ScopedTestEnvVar::set(super::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1")
}

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
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        config: Some(&config),
        force: true, // force tier bypass
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
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
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        config: Some(&config),
        force: true,                      // force tier bypass
        force_override_user_config: true, // override enabled
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
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
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    let result = super::resolve_tool_and_model(super::RoutingRequest {
        model_spec: Some("codex/openai/gpt-5.3-codex/high"),
        config: Some(&config),
        force: true,
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("disabled in user configuration"), "{msg}");
}

// --- resolve_tool_from_tier tests ---

pub(super) fn config_with_tier(
    tier_name: &str,
    models: Vec<&str>,
    enabled_tools: &[&str],
) -> ProjectConfig {
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
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

#[test]
fn resolve_tool_from_tier_returns_none_for_missing_tier() {
    let cfg = config_with_tier(
        "tier-1",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli"],
    );
    let result = super::resolve_tool_from_tier("nonexistent-tier", &cfg, None, None, &[]);
    assert!(result.is_none());
}

#[test]
fn resolve_tool_from_tier_returns_first_available_when_no_parent() {
    let _tool_availability = assume_tier_tools_available();
    let cfg = config_with_tier(
        "test-tier",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli"],
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, None, None, &[]);
    assert!(result.is_some());
    let res = result.unwrap();
    assert_eq!(res.tool, ToolName::GeminiCli);
    assert_eq!(res.model_spec, "gemini-cli/google/default/xhigh");
}

#[test]
fn resolve_tool_from_tier_prefers_heterogeneous() {
    let _tool_availability = assume_tier_tools_available();
    // Parent=claude-code(Anthropic), tier has gemini-cli+claude-code; prefer gemini for heterogeneity.
    let cfg = config_with_tier(
        "test-tier",
        vec![
            "claude-code/anthropic/default/xhigh",
            "gemini-cli/google/default/xhigh",
        ],
        &["claude-code", "gemini-cli"],
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, Some("claude-code"), None, &[]);
    assert!(result.is_some());
    let res = result.unwrap();
    assert_eq!(res.tool, ToolName::GeminiCli);
    assert_eq!(res.model_spec, "gemini-cli/google/default/xhigh");
}

#[test]
fn resolve_tool_from_tier_falls_back_to_same_family_when_no_heterogeneous() {
    let _tool_availability = assume_tier_tools_available();
    // Parent is claude-code, tier only has claude-code models.
    // No heterogeneous option — should still return the first available.
    let cfg = config_with_tier(
        "test-tier",
        vec!["claude-code/anthropic/default/xhigh"],
        &["claude-code"],
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, Some("claude-code"), None, &[]);
    assert!(result.is_some());
    let res = result.unwrap();
    assert_eq!(res.tool, ToolName::ClaudeCode);
}

#[test]
fn resolve_tool_from_tier_skips_disabled_tools() {
    let _tool_availability = assume_tier_tools_available();
    // gemini-cli is disabled, only claude-code is enabled.
    let cfg = config_with_tier(
        "test-tier",
        vec![
            "gemini-cli/google/default/xhigh",
            "claude-code/anthropic/default/xhigh",
        ],
        &["claude-code"],
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, None, None, &[]);
    assert!(result.is_some());
    let res = result.unwrap();
    assert_eq!(res.tool, ToolName::ClaudeCode);
}

#[test]
fn resolve_tool_from_tier_returns_none_when_all_disabled() {
    let _tool_availability = assume_tier_tools_available();
    // All tools in tier are disabled.
    let cfg = config_with_tier(
        "test-tier",
        vec!["gemini-cli/google/default/xhigh"],
        &[], // no enabled tools
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, None, None, &[]);
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
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::GeminiCli),
        config: Some(&cfg),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
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

/// When tiers are configured, --model-spec is the exact-selection escape hatch and
/// implicitly bypasses the tier whitelist. `enforce_tool_enabled` still applies (see
/// `resolve_tool_and_model_disabled_tool_model_spec_errors`).
#[test]
fn resolve_tool_and_model_allows_model_spec_exact_selection_when_tiers_configured() {
    let cfg = config_with_tier(
        "default",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
    );
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        model_spec: Some("codex/openai/gpt-5.4/high"),
        config: Some(&cfg),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(
        result.is_ok(),
        "model-spec should bypass tier whitelist: {}",
        result.unwrap_err()
    );
    let (tool, model_spec, model) = result.unwrap();
    assert_eq!(tool, ToolName::Codex);
    assert_eq!(model_spec.as_deref(), Some("codex/openai/gpt-5.4/high"));
    assert!(model.is_none());
}

/// When tiers are configured, direct --model alone (no --tool, no --model-spec) is blocked.
#[test]
fn resolve_tool_and_model_blocks_direct_model_alone_when_tiers_configured() {
    let cfg = config_with_tier(
        "default",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli"],
    );
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        model: Some("custom-model"), // --model provided
        config: Some(&cfg),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
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
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        model: Some("gpt-4"),   // model required when bypassing tiers
        thinking: Some("high"), // thinking required when bypassing tiers
        config: Some(&cfg),
        force_ignore_tier_setting: true, // force_ignore_tier_setting
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
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
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        config: Some(&cfg),
        force_override_user_config: true, // force_override_user_config → bypasses tier enforcement
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
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
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        config: Some(&cfg),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
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
    let _tool_availability = assume_tier_tools_available();
    let cfg = config_with_tier(
        "quality",
        vec!["gemini-cli/google/gemini-3.1-pro-preview/xhigh"],
        &["gemini-cli"],
    );
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        config: Some(&cfg),
        tier: Some("quality"), // --tier quality
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
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

#[test]
fn resolve_tool_and_model_tier_shorthand_resolves_from_tier() {
    let _tool_availability = assume_tier_tools_available();
    let cfg = config_with_tier(
        "tier-4-critical",
        vec!["gemini-cli/google/gemini-3.1-pro-preview/xhigh"],
        &["gemini-cli"],
    );

    for selector in ["tier4", "tier-4"] {
        let result = super::resolve_tool_and_model(super::RoutingRequest {
            config: Some(&cfg),
            tier: Some(selector),
            ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
        });
        assert!(
            result.is_ok(),
            "{selector} tier resolution failed: {}",
            result.unwrap_err()
        );
        let (tool, model_spec, _) = result.unwrap();
        assert_eq!(tool, ToolName::GeminiCli);
        assert_eq!(
            model_spec.as_deref(),
            Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh")
        );
    }
}

#[test]
fn resolve_tool_and_model_tier_with_tool_resolves_requested_tool_from_tier() {
    let _tool_availability = assume_tier_tools_available();
    let cfg = config_with_tier(
        "tier-4-critical",
        vec![
            "gemini-cli/google/default/xhigh",
            "codex/openai/gpt-5.4/xhigh",
            "claude-code/anthropic/sonnet-4.6/xhigh",
        ],
        &["gemini-cli", "codex", "claude-code"],
    );
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        config: Some(&cfg),
        tier: Some("tier-4-critical"),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(
        result.is_ok(),
        "tier+tool should resolve requested tool from tier: {}",
        result.unwrap_err()
    );
    let (tool, model_spec, _) = result.unwrap();
    assert_eq!(tool, ToolName::Codex);
    assert_eq!(model_spec.as_deref(), Some("codex/openai/gpt-5.4/xhigh"));
}

#[test]
fn resolve_tool_and_model_tier_with_tool_errors_when_tool_missing_from_tier() {
    let cfg = config_with_tier(
        "quality",
        vec![
            "gemini-cli/google/default/xhigh",
            "claude-code/anthropic/sonnet-4.6/xhigh",
        ],
        &["gemini-cli", "codex", "claude-code"],
    );

    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        config: Some(&cfg),
        tier: Some("quality"),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });

    let err = result.expect_err("missing tool in tier must error");
    let msg = err.to_string();
    assert!(
        msg.contains("Tool 'codex' is not available in tier 'quality'"),
        "unexpected error: {msg}"
    );
    assert!(
        msg.contains("Available tools in tier 'quality'"),
        "unexpected error: {msg}"
    );
}

#[test]
fn resolve_tool_and_model_tier_ignores_auto_resolved_tool_hint() {
    let _tool_availability = assume_tier_tools_available();
    let cfg = config_with_tier(
        "quality",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
    );

    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        config: Some(&cfg),
        tier: Some("quality"),
        tool_is_auto_resolved: true, // tool_is_auto_resolved
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });

    assert!(
        result.is_ok(),
        "auto-resolved tool hint should not constrain tier: {}",
        result.unwrap_err()
    );
    let (tool, model_spec, _) = result.unwrap();
    assert_eq!(tool, ToolName::GeminiCli);
    assert_eq!(
        model_spec.as_deref(),
        Some("gemini-cli/google/default/xhigh")
    );
}

#[test]
fn resolve_tool_and_model_tier_with_tool_and_force_ignore_errors_on_conflict() {
    let cfg = config_with_tier(
        "quality",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli", "codex"],
    );

    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        config: Some(&cfg),
        tier: Some("quality"),
        force_ignore_tier_setting: true,
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });

    let err = result.expect_err("tier + direct tool bypass must be explicit, not silent");
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

// --- tier_mapping alias tests ---

#[test]
fn resolve_tool_and_model_tier_alias_resolves_correctly() {
    let _tool_availability = assume_tier_tools_available();
    let mut cfg = config_with_tier(
        "tier1",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli"],
    );
    cfg.tier_mapping
        .insert("default".to_string(), "tier1".to_string());

    let result = super::resolve_tool_and_model(super::RoutingRequest {
        config: Some(&cfg),
        tier: Some("default"), // tier_mapping alias for tier1
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(
        result.is_ok(),
        "alias should resolve: {}",
        result.unwrap_err()
    );
    let (tool, model_spec, _) = result.unwrap();
    assert_eq!(tool, ToolName::GeminiCli);
    assert_eq!(
        model_spec.as_deref(),
        Some("gemini-cli/google/default/xhigh")
    );
}

#[test]
fn resolve_tool_and_model_invalid_tier_selector_includes_aliases_in_error() {
    let mut cfg = config_with_tier(
        "tier1",
        vec!["gemini-cli/google/default/xhigh"],
        &["gemini-cli"],
    );
    cfg.tier_mapping
        .insert("alias1".to_string(), "tier1".to_string());

    let result = super::resolve_tool_and_model(super::RoutingRequest {
        config: Some(&cfg),
        tier: Some("invalid"),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Tier selector 'invalid' not found"),
        "msg: {msg}"
    );
    assert!(msg.contains("Available tiers:"), "msg: {msg}");
    assert!(
        msg.contains("Available tier aliases:"),
        "msg should show aliases: {msg}"
    );
}
