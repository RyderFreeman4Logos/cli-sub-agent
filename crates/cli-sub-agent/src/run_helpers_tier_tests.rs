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

#[test]
fn resolve_tool_from_tier_returns_none_for_missing_tier() {
    let cfg = config_with_tier("tier-1", vec!["opencode/openai/gpt-5/xhigh"], &["opencode"]);
    let result = super::resolve_tool_from_tier("nonexistent-tier", &cfg, None, &[], &[]);
    assert!(result.is_none());
}

#[test]
fn resolve_tool_from_tier_returns_first_available_when_no_parent() {
    let _tool_availability = assume_tier_tools_available();
    let cfg = config_with_tier(
        "test-tier",
        vec!["opencode/openai/gpt-5/xhigh"],
        &["opencode"],
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, None, &[], &[]);
    assert!(result.is_some());
    let res = result.unwrap();
    assert_eq!(res.tool, ToolName::Opencode);
    assert_eq!(res.model_spec, "opencode/openai/gpt-5/xhigh");
}

#[test]
fn resolve_tool_from_tier_prefers_heterogeneous() {
    let _tool_availability = assume_tier_tools_available();
    // Parent=claude-code(Anthropic), tier has opencode+claude-code; prefer opencode for heterogeneity.
    let cfg = config_with_tier(
        "test-tier",
        vec![
            "claude-code/anthropic/default/xhigh",
            "opencode/openai/gpt-5/xhigh",
        ],
        &["claude-code", "opencode"],
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, Some("claude-code"), &[], &[]);
    assert!(result.is_some());
    let res = result.unwrap();
    assert_eq!(res.tool, ToolName::Opencode);
    assert_eq!(res.model_spec, "opencode/openai/gpt-5/xhigh");
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
    let result = super::resolve_tool_from_tier("test-tier", &cfg, Some("claude-code"), &[], &[]);
    assert!(result.is_some());
    let res = result.unwrap();
    assert_eq!(res.tool, ToolName::ClaudeCode);
}

#[test]
fn resolve_tool_from_tier_skips_disabled_tools() {
    let _tool_availability = assume_tier_tools_available();
    // opencode is disabled, only claude-code is enabled.
    let cfg = config_with_tier(
        "test-tier",
        vec![
            "opencode/openai/gpt-5/xhigh",
            "claude-code/anthropic/default/xhigh",
        ],
        &["claude-code"],
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, None, &[], &[]);
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
        vec!["opencode/openai/gpt-5/xhigh"],
        &[], // no enabled tools
    );
    let result = super::resolve_tool_from_tier("test-tier", &cfg, None, &[], &[]);
    assert!(result.is_none());
}

// --- resolve_preferred_tool_from_tier (explicit --tool pin) tests ---

#[test]
fn resolve_preferred_tool_from_tier_fails_fast_on_disabled_pinned_candidate() {
    let _tool_availability = assume_tier_tools_available();
    // claude-code IS a tier candidate but is disabled in config; opencode is
    // the enabled tier default. An explicit `--tool claude-code` pin must error
    // instead of silently falling through to opencode (#1836). Resolution is
    // upstream of failover, so this fail-fast fires regardless of --no-failover.
    let cfg = config_with_tier(
        "tier-4-critical",
        vec![
            "opencode/openai/gpt-5/xhigh",
            "claude-code/anthropic/default/xhigh",
        ],
        &["opencode"], // claude-code disabled
    );
    let preference_order = vec!["claude-code".to_string()];
    let result = super::resolve_preferred_tool_from_tier(
        "tier-4-critical",
        &cfg,
        None,
        &preference_order,
        &[],
    );
    let err = result.expect_err("disabled pinned tier candidate must fail fast");
    let msg = err.to_string();
    assert!(msg.contains("--tool claude-code requested"), "{msg}");
    assert!(msg.contains("[tools.claude-code].enabled = false"), "{msg}");
    assert!(
        msg.contains("enable it") && msg.contains("choose an enabled tool"),
        "{msg}"
    );
}

#[test]
fn resolve_preferred_tool_from_tier_soft_reorders_enabled_candidate() {
    let _tool_availability = assume_tier_tools_available();
    // opencode is the tier default, but the user pins the ENABLED candidate
    // codex; the soft-reorder must surface codex without error (#1749 preserved,
    // regression guard — the disabled fail-fast must not catch enabled pins).
    let cfg = config_with_tier(
        "tier-4-critical",
        vec!["opencode/openai/gpt-5/xhigh", "codex/openai/gpt-5.5/xhigh"],
        &["opencode", "codex"],
    );
    let preference_order = vec!["codex".to_string()];
    let resolution = super::resolve_preferred_tool_from_tier(
        "tier-4-critical",
        &cfg,
        None,
        &preference_order,
        &[],
    )
    .expect("enabled pinned candidate must resolve via soft-reorder");
    assert_eq!(resolution.tool, ToolName::Codex);
    assert_eq!(resolution.model_spec, "codex/openai/gpt-5.5/xhigh");
}

#[test]
fn resolve_preferred_tool_from_tier_rejects_non_candidate_pin() {
    let _tool_availability = assume_tier_tools_available();
    // opencode is enabled but NOT a tier candidate. Since #1994, non-candidate
    // pins fail fast instead of silently substituting a different tool.
    let cfg = config_with_tier(
        "tier-4-critical",
        vec!["codex/openai/gpt-5.5/xhigh"],
        &["opencode", "codex"],
    );
    let preference_order = vec!["opencode".to_string()];
    let err = super::resolve_preferred_tool_from_tier(
        "tier-4-critical",
        &cfg,
        None,
        &preference_order,
        &[],
    )
    .expect_err("non-candidate pin must fail fast since #1994");
    let msg = err.to_string();
    assert!(
        msg.contains("opencode") && msg.contains("not a candidate"),
        "error should name the rejected tool: {msg}"
    );
}

/// End-to-end wiring guard: the `csa run` resolution entry point routes an
/// explicit `--tool <disabled candidate>` + `--tier` through the fail-fast
/// (#1836), proving the silent fall-through to the tier default is closed.
#[test]
fn resolve_tool_and_model_fails_fast_on_disabled_pinned_tier_candidate() {
    let _tool_availability = assume_tier_tools_available();
    let cfg = config_with_tier(
        "tier-4-critical",
        vec![
            "opencode/openai/gpt-5/xhigh",
            "claude-code/anthropic/default/xhigh",
        ],
        &["opencode"], // claude-code disabled
    );
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::ClaudeCode),
        tier: Some("tier-4-critical"),
        config: Some(&cfg),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    let err = result.expect_err("disabled pinned tool under a tier must fail fast");
    let msg = err.to_string();
    assert!(msg.contains("--tool claude-code requested"), "{msg}");
    assert!(msg.contains("[tools.claude-code].enabled = false"), "{msg}");
}

// --- Phase 2: tier enforcement tests ---

/// When tiers are configured, direct --tool without an active tier is blocked.
#[test]
fn resolve_tool_and_model_blocks_direct_tool_when_tiers_configured() {
    let cfg = config_with_tier(
        "default",
        vec!["opencode/openai/gpt-5/xhigh"],
        &["opencode"],
    );
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Opencode),
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
        msg.contains("[tier_policy].allow_force_bypass"),
        "should mention global escape hatch: {msg}"
    );
    assert!(
        !msg.contains("--force-ignore-tier-setting"),
        "direct-tool guard should not recommend bypass flags as normal remediation: {msg}"
    );
}

/// When tiers are configured, --model-spec must match one configured tier model.
#[test]
fn resolve_tool_and_model_rejects_unconfigured_model_spec_when_tiers_configured() {
    let cfg = config_with_tier(
        "default",
        vec!["opencode/openai/gpt-5/xhigh"],
        &["opencode", "codex"],
    );
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        model_spec: Some("codex/openai/gpt-5.4/high"),
        config: Some(&cfg),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(result.is_err(), "model-spec must not bypass tier whitelist");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("not configured in any tier"),
        "unexpected error: {msg}"
    );
}

#[test]
fn resolve_tool_and_model_allows_configured_model_spec_when_tiers_configured() {
    let cfg = config_with_tier(
        "default",
        vec!["codex/openai/gpt-5.5/high"],
        &["opencode", "codex"],
    );
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        model_spec: Some("codex/openai/gpt-5.5/high"),
        config: Some(&cfg),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(
        result.is_ok(),
        "configured model-spec should pass: {}",
        result.unwrap_err()
    );
    let (tool, model_spec, model) = result.unwrap();
    assert_eq!(tool, ToolName::Codex);
    assert_eq!(model_spec.as_deref(), Some("codex/openai/gpt-5.5/high"));
    assert!(model.is_none());
}

/// When tiers are configured, direct --model alone (no --tool, no --model-spec) is blocked.
#[test]
fn resolve_tool_and_model_blocks_direct_model_alone_when_tiers_configured() {
    let cfg = config_with_tier(
        "default",
        vec!["opencode/openai/gpt-5/xhigh"],
        &["opencode"],
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

/// When tiers are configured, direct --thinking alone is blocked.
#[test]
fn resolve_tool_and_model_blocks_direct_thinking_alone_when_tiers_configured() {
    let cfg = config_with_tier(
        "default",
        vec!["opencode/openai/gpt-5/xhigh"],
        &["opencode"],
    );
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        thinking: Some("low"),
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
        vec!["opencode/openai/gpt-5/xhigh"],
        &["opencode", "codex"],
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

/// --force-override-user-config only bypasses tool enablement, not tier enforcement.
#[test]
fn resolve_tool_and_model_force_override_user_config_does_not_bypass_tiers() {
    let cfg = config_with_tier(
        "default",
        vec!["opencode/openai/gpt-5/xhigh"],
        &["opencode", "codex"],
    );
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        config: Some(&cfg),
        force_override_user_config: true,
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(result.is_err(), "force-override should not bypass tiers");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Direct --tool/--model/--thinking is restricted"),
        "{msg}"
    );
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
        vec!["opencode/openai/gpt-5/xhigh"],
        &["opencode"],
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
    assert_eq!(tool, ToolName::Opencode);
    assert_eq!(model_spec.as_deref(), Some("opencode/openai/gpt-5/xhigh"));
}

#[test]
fn resolve_tool_and_model_tier_shorthand_resolves_from_tier() {
    let _tool_availability = assume_tier_tools_available();
    let cfg = config_with_tier(
        "tier-4-critical",
        vec!["opencode/openai/gpt-5/xhigh"],
        &["opencode"],
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
        assert_eq!(tool, ToolName::Opencode);
        assert_eq!(model_spec.as_deref(), Some("opencode/openai/gpt-5/xhigh"));
    }
}

#[test]
fn resolve_tool_and_model_tier_with_tool_resolves_requested_tool_from_tier() {
    let _tool_availability = assume_tier_tools_available();
    let cfg = config_with_tier(
        "tier-4-critical",
        vec![
            "opencode/openai/gpt-5/xhigh",
            "codex/openai/gpt-5.4/xhigh",
            "claude-code/anthropic/sonnet-4.6/xhigh",
        ],
        &["opencode", "codex", "claude-code"],
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
fn resolve_tool_and_model_tier_with_tool_rejects_missing_tool_candidate() {
    let _tool_availability = assume_tier_tools_available();
    let cfg = config_with_tier(
        "quality",
        vec![
            "opencode/openai/gpt-5/xhigh",
            "claude-code/anthropic/sonnet-4.6/xhigh",
        ],
        &["opencode", "codex", "claude-code"],
    );
    let err = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        config: Some(&cfg),
        tier: Some("quality"),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    })
    .expect_err("non-candidate --tool pin must fail fast since #1994");
    let msg = err.to_string();
    assert!(
        msg.contains("codex") && msg.contains("not a candidate"),
        "error should name the rejected tool: {msg}"
    );
}

#[test]
fn resolve_tool_and_model_tier_ignores_auto_resolved_tool_hint() {
    let _tool_availability = assume_tier_tools_available();
    let cfg = config_with_tier(
        "quality",
        vec!["opencode/openai/gpt-5/xhigh"],
        &["opencode", "codex"],
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
    assert_eq!(tool, ToolName::Opencode);
    assert_eq!(model_spec.as_deref(), Some("opencode/openai/gpt-5/xhigh"));
}

#[test]
fn resolve_tool_and_model_tier_with_tool_and_force_ignore_errors_on_conflict() {
    let cfg = config_with_tier(
        "quality",
        vec!["opencode/openai/gpt-5/xhigh"],
        &["opencode", "codex"],
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
    let mut cfg = config_with_tier("tier1", vec!["opencode/openai/gpt-5/xhigh"], &["opencode"]);
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
    assert_eq!(tool, ToolName::Opencode);
    assert_eq!(model_spec.as_deref(), Some("opencode/openai/gpt-5/xhigh"));
}

#[test]
fn resolve_tool_and_model_invalid_tier_selector_includes_aliases_in_error() {
    let mut cfg = config_with_tier("tier1", vec!["opencode/openai/gpt-5/xhigh"], &["opencode"]);
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

// --- tier_without_tool_should_warn tests ---

#[test]
fn tier_without_tool_warns_when_tier_set_and_no_tool() {
    assert!(super::tier_without_tool_should_warn(
        Some("tier-1-quick"),
        false
    ));
}

#[test]
fn tier_without_tool_no_warn_when_tool_explicitly_set() {
    assert!(!super::tier_without_tool_should_warn(
        Some("tier-1-quick"),
        true
    ));
}

#[test]
fn tier_without_tool_no_warn_when_no_tier() {
    assert!(!super::tier_without_tool_should_warn(None, false));
}

#[test]
fn tier_without_tool_no_warn_when_neither() {
    assert!(!super::tier_without_tool_should_warn(None, true));
}
