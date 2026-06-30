use crate::test_env_lock::ScopedTestEnvVar;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use std::collections::HashMap;

use super::tier_tests::config_with_tier;

fn assume_tier_tools_available() -> ScopedTestEnvVar {
    ScopedTestEnvVar::set(super::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1")
}

#[test]
fn tier_bypass_gate_allows_bypass_flags_when_no_tiers_configured() {
    let cfg = ProjectConfig {
        schema_version: 1,
        project: Default::default(),
        resources: Default::default(),
        acp: Default::default(),
        tools: HashMap::new(),
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
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    super::enforce_tier_bypass_gate(super::TierBypassGateCtx {
        project_config: Some(&cfg),
        global_config: &GlobalConfig::default(),
        flags: super::TierBypassGateFlags {
            model_spec: true,
            force: true,
            force_ignore_tier_setting: true,
            model: true,
            thinking: true,
        },
        inherited_trusted_pin: false,
    })
    .expect("no tiers configured should preserve exact/force bypass behavior");
}

#[test]
fn tier_bypass_gate_allows_bypass_flags_with_global_opt_in() {
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-5.5/high"], &["codex"]);
    let global = GlobalConfig {
        tier_policy: csa_config::TierPolicyConfig {
            allow_force_bypass: true,
        },
        ..Default::default()
    };

    super::enforce_tier_bypass_gate(super::TierBypassGateCtx {
        project_config: Some(&cfg),
        global_config: &global,
        flags: super::TierBypassGateFlags {
            model_spec: true,
            force: true,
            force_ignore_tier_setting: true,
            model: true,
            thinking: true,
        },
        inherited_trusted_pin: false,
    })
    .expect("global opt-in should allow emergency exact/force bypasses");
}

#[test]
fn tier_bypass_gate_rejects_model_spec_and_force_by_default() {
    let mut cfg = config_with_tier(
        "tier-2-standard",
        vec!["codex/openai/gpt-5.5/high"],
        &["codex"],
    );
    cfg.tiers.insert(
        "tier-4-critical".to_string(),
        csa_config::TierConfig {
            description: "critical".to_string(),
            models: vec!["opencode/openai/gpt-5/high".to_string()],
            strategy: Default::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    let err = super::enforce_tier_bypass_gate(super::TierBypassGateCtx {
        project_config: Some(&cfg),
        global_config: &GlobalConfig::default(),
        flags: super::TierBypassGateFlags {
            model_spec: true,
            force: false,
            force_ignore_tier_setting: true,
            model: false,
            thinking: false,
        },
        inherited_trusted_pin: false,
    })
    .expect_err("tier bypass should be gated by default when tiers exist");
    let msg = err.to_string();

    assert!(msg.contains("Tier bypass is disabled because [tiers] are configured"));
    assert!(msg.contains("Use --tier <name>"));
    assert!(msg.contains("Available tiers: [tier-2-standard, tier-4-critical]"));
    assert!(msg.contains("[tier_policy].allow_force_bypass = true"));
    assert!(msg.contains("manual escape hatch for total-exhaustion situations"));
    assert!(msg.contains("CSA will not auto-enable it or auto-admit excluded tools"));
    assert!(msg.contains("Refused flags: --model-spec, --force-ignore-tier-setting"));
}

#[test]
fn tier_bypass_gate_rejects_all_gated_flags_by_default() {
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-5.5/high"], &["codex"]);

    let err = super::enforce_tier_bypass_gate(super::TierBypassGateCtx {
        project_config: Some(&cfg),
        global_config: &GlobalConfig::default(),
        flags: super::TierBypassGateFlags {
            model_spec: true,
            force: true,
            force_ignore_tier_setting: true,
            model: true,
            thinking: true,
        },
        inherited_trusted_pin: false,
    })
    .expect_err("all gated flags should be rejected by default when tiers exist");
    let msg = err.to_string();

    assert!(
        msg.contains(
            "Refused flags: --model-spec, --force, --force-ignore-tier-setting, --model, --thinking"
        ),
        "{msg}"
    );
}

#[test]
fn tier_bypass_gate_allows_inherited_trusted_pin() {
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-5.5/high"], &["codex"]);

    super::enforce_tier_bypass_gate(super::TierBypassGateCtx {
        project_config: Some(&cfg),
        global_config: &GlobalConfig::default(),
        flags: super::TierBypassGateFlags {
            model_spec: true,
            force: false,
            force_ignore_tier_setting: true,
            model: false,
            thinking: false,
        },
        inherited_trusted_pin: true,
    })
    .expect("trusted inherited #1741 subtree pins should continue under gate-off");
}

#[test]
fn tier_bypass_gate_rejects_user_force_with_inherited_trusted_pin() {
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-5.5/high"], &["codex"]);

    let err = super::enforce_tier_bypass_gate(super::TierBypassGateCtx {
        project_config: Some(&cfg),
        global_config: &GlobalConfig::default(),
        flags: super::TierBypassGateFlags {
            model_spec: true,
            force: true,
            force_ignore_tier_setting: true,
            model: false,
            thinking: false,
        },
        inherited_trusted_pin: true,
    })
    .expect_err("inherited subtree pins must not allow unrelated user bypass flags");
    let msg = err.to_string();

    assert!(msg.contains("Refused flags: --force"), "{msg}");
    assert!(!msg.contains("--model-spec"), "{msg}");
    assert!(!msg.contains("--force-ignore-tier-setting"), "{msg}");
}

#[test]
fn resolve_tool_and_model_force_ignore_tier_requires_complete_spec() {
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-5.5/high"], &["codex"]);

    // Missing all required flags
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        config: Some(&cfg),
        force_ignore_tier_setting: true, // force_ignore_tier_setting = true
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("When using --force-ignore-tier-setting"),
        "msg: {msg}"
    );
    assert!(
        msg.contains("Missing required flags: --tool, --model, --thinking"),
        "msg: {msg}"
    );
    assert!(
        msg.contains("Example: csa run --sa-mode <true|false> --force-ignore-tier-setting"),
        "msg: {msg}"
    );

    // Missing only --thinking
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex), // --tool provided
        model: Some("gpt-4"),        // --model provided
        config: Some(&cfg),
        force_ignore_tier_setting: true, // force_ignore_tier_setting = true
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Missing required flags: --thinking"),
        "msg: {msg}"
    );

    // Missing only --model
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex), // --tool provided
        thinking: Some("high"),      // --thinking provided
        config: Some(&cfg),
        force_ignore_tier_setting: true, // force_ignore_tier_setting = true
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Missing required flags: --model"),
        "msg: {msg}"
    );

    // Missing only --tool
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        model: Some("gpt-4"),   // --model provided
        thinking: Some("high"), // --thinking provided
        config: Some(&cfg),
        force_ignore_tier_setting: true, // force_ignore_tier_setting = true
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("Missing required flags: --tool"), "msg: {msg}");
}

#[test]
fn resolve_tool_and_model_force_ignore_tier_allows_complete_spec() {
    let _guard = assume_tier_tools_available();
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-5.5/high"], &["codex"]);

    // All required flags provided - should succeed
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex), // --tool provided
        model: Some("gpt-4"),        // --model provided
        thinking: Some("high"),      // --thinking provided
        config: Some(&cfg),
        force_ignore_tier_setting: true, // force_ignore_tier_setting = true
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(
        result.is_ok(),
        "Complete spec should be allowed: {:?}",
        result
    );
    let (tool, model_spec, model) = result.unwrap();
    assert_eq!(tool, ToolName::Codex);
    assert_eq!(model_spec, None);
    assert_eq!(model, Some("gpt-4".to_string()));
}

#[test]
fn resolve_tool_and_model_force_ignore_tier_uses_tool_defaults() {
    let _guard = assume_tier_tools_available();
    let mut cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-5.5/high"], &["codex"]);
    let codex = cfg
        .tools
        .get_mut("codex")
        .expect("config_with_tier should create codex tool config");
    codex.default_model = Some("gpt-5.4".to_string());
    codex.default_thinking = Some("xhigh".to_string());

    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        config: Some(&cfg),
        force_ignore_tier_setting: true,
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    let (tool, model_spec, model) = result.expect("configured tool defaults should satisfy bypass");
    assert_eq!(tool, ToolName::Codex);
    assert_eq!(model_spec, None);
    assert_eq!(model, None);

    let executor = super::build_executor(
        &tool,
        model_spec.as_deref(),
        model.as_deref(),
        None,
        Some(&cfg),
        true,
    )
    .expect("run execution should apply configured tool defaults");
    let debug = format!("{executor:?}");
    assert!(debug.contains("gpt-5.4"), "default model missing: {debug}");
    assert!(debug.contains("Xhigh"), "default thinking missing: {debug}");
}

#[test]
fn resolve_tool_and_model_force_ignore_tier_bypassed_when_tier_provided() {
    let _guard = assume_tier_tools_available();
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-5.5/high"], &["codex"]);

    // When --tier is provided, validation should be skipped
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        config: Some(&cfg),
        tier: Some("tier-1"),            // --tier provided
        force_ignore_tier_setting: true, // force_ignore_tier_setting = true
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    // Should succeed because tier is provided, bypassing the validation
    assert!(
        result.is_ok(),
        "Tier provided should bypass validation: {:?}",
        result
    );
}

#[test]
fn resolve_tool_and_model_force_ignore_tier_bypassed_when_model_spec_provided() {
    let _guard = assume_tier_tools_available();
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-5.5/high"], &["codex"]);

    // --force-ignore-tier-setting preserves the explicit bypass for unconfigured specs.
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        model_spec: Some("codex/openai/gpt-4/high"), // --model-spec provided
        config: Some(&cfg),
        force_ignore_tier_setting: true, // force_ignore_tier_setting = true
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(
        result.is_ok(),
        "force-ignore should bypass model_spec validation: {:?}",
        result
    );
}

#[test]
fn resolve_tool_and_model_allows_model_spec_when_global_tier_bypass_opted_in() {
    let cfg = config_with_tier(
        "tier-1",
        vec!["opencode/openai/gpt-5/xhigh"],
        &["opencode", "codex"],
    );
    let global = GlobalConfig {
        tier_policy: csa_config::TierPolicyConfig {
            allow_force_bypass: true,
        },
        ..Default::default()
    };

    super::enforce_tier_bypass_gate(super::TierBypassGateCtx {
        project_config: Some(&cfg),
        global_config: &global,
        flags: super::TierBypassGateFlags {
            model_spec: true,
            force: false,
            force_ignore_tier_setting: false,
            model: false,
            thinking: false,
        },
        inherited_trusted_pin: false,
    })
    .expect("global opt-in should allow bare --model-spec tier bypass");

    let result = super::resolve_tool_and_model(super::RoutingRequest {
        model_spec: Some("codex/openai/gpt-5.4/high"),
        config: Some(&cfg),
        tier_bypass_allowed: super::tier_bypass_allowed(Some(&cfg), &global, false),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    })
    .expect("opted-in bare --model-spec should resolve exact model");

    assert_eq!(result.0, ToolName::Codex);
    assert_eq!(result.1.as_deref(), Some("codex/openai/gpt-5.4/high"));
    assert!(result.2.is_none());
}

#[test]
fn resolve_tool_and_model_uses_inherited_model_spec_when_gate_default() {
    let _guard = assume_tier_tools_available();
    let cfg = config_with_tier(
        "tier-1",
        vec!["opencode/openai/gpt-5/xhigh"],
        &["opencode", "codex"],
    );
    let global = GlobalConfig::default();
    let inherited_spec = "codex/openai/gpt-5.5/xhigh";

    super::enforce_tier_bypass_gate(super::TierBypassGateCtx {
        project_config: Some(&cfg),
        global_config: &global,
        flags: super::TierBypassGateFlags {
            model_spec: true,
            force: false,
            force_ignore_tier_setting: true,
            model: false,
            thinking: false,
        },
        inherited_trusted_pin: true,
    })
    .expect("inherited subtree model-spec should pass the gate without global opt-in");

    let result = super::resolve_tool_and_model(super::RoutingRequest {
        model_spec: Some(inherited_spec),
        config: Some(&cfg),
        force_ignore_tier_setting: true,
        tier_bypass_allowed: super::tier_bypass_allowed(Some(&cfg), &global, true),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    })
    .expect("trusted inherited model-spec should resolve exactly");

    assert_eq!(result.0, ToolName::Codex);
    assert_eq!(result.1.as_deref(), Some(inherited_spec));
    assert!(result.2.is_none());
}

#[test]
fn resolve_tool_and_model_allows_thinking_when_global_tier_bypass_opted_in() {
    let _guard = assume_tier_tools_available();
    let mut cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-5.4/high"], &["codex"]);
    cfg.tier_mapping
        .insert("default".to_string(), "tier-1".to_string());
    let global = GlobalConfig {
        tier_policy: csa_config::TierPolicyConfig {
            allow_force_bypass: true,
        },
        ..Default::default()
    };

    let bypass_allowed = super::tier_bypass_allowed(Some(&cfg), &global, false);
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        thinking: Some("low"),
        config: Some(&cfg),
        tier_bypass_allowed: bypass_allowed,
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    })
    .expect("opted-in bare --thinking should resolve through the default tier");

    assert_eq!(result.0, ToolName::Codex);
    assert_eq!(result.1.as_deref(), Some("codex/openai/gpt-5.4/high"));
}

#[test]
fn collect_preferred_tier_models_honors_preference_array_order() {
    let _guard = assume_tier_tools_available();
    let cfg = config_with_tier(
        "quality",
        vec![
            "opencode/openai/gpt-5/xhigh",
            "codex/openai/gpt-5.4/high",
            "claude-code/anthropic/sonnet-4.5/high",
        ],
        &["opencode", "codex", "claude-code"],
    );
    let preference_order = vec!["codex".to_string(), "opencode".to_string()];

    let candidates = super::collect_preferred_tier_models("quality", &cfg, &preference_order, &[]);
    let specs: Vec<&str> = candidates
        .iter()
        .map(|resolution| resolution.model_spec.as_str())
        .collect();

    assert_eq!(
        specs,
        vec![
            "codex/openai/gpt-5.4/high",
            "opencode/openai/gpt-5/xhigh",
            "claude-code/anthropic/sonnet-4.5/high",
        ]
    );
}

#[test]
fn resolve_tool_and_model_force_ignore_tier_skipped_when_no_tiers_configured() {
    let cfg = ProjectConfig {
        schema_version: 1,
        project: Default::default(),
        resources: Default::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(), // No tiers configured
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
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    // Use an explicit tool so the test only exercises the no-tiers force-ignore
    // validation path, not host-dependent auto-selection of installed tools.
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        config: Some(&cfg),
        force_ignore_tier_setting: true, // force_ignore_tier_setting = true
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    let (tool, model_spec, model) =
        result.expect("no tiers configured should skip force-ignore complete-spec validation");

    assert_eq!(tool, ToolName::Codex);
    assert_eq!(model_spec, None);
    assert_eq!(model, None);
}

#[test]
fn resolve_tool_and_model_force_ignore_tier_skipped_when_flag_false() {
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-5.5/high"], &["codex"]);

    // When force_ignore_tier_setting = false, validation should be skipped
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        config: Some(&cfg),
        force_ignore_tier_setting: false, // force_ignore_tier_setting = false
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    // Should fail for different reason (tier enforcement), but not our new validation
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    // Should be the original tier enforcement error, not our new validation
    assert!(
        !msg.contains("When using --force-ignore-tier-setting"),
        "Should not trigger new validation when flag is false: {msg}"
    );
}
