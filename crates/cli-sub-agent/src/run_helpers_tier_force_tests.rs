use crate::test_env_lock::ScopedTestEnvVar;
use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use std::collections::HashMap;

use super::tier_tests::config_with_tier;

fn assume_tier_tools_available() -> ScopedTestEnvVar {
    ScopedTestEnvVar::set(super::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1")
}

#[test]
fn resolve_tool_and_model_force_ignore_tier_requires_complete_spec() {
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-4/high"], &["codex"]);

    // Missing all required flags
    let result = super::resolve_tool_and_model(
        None, // missing --tool
        None, // no --model-spec
        None, // missing --model
        None, // missing --thinking
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        None, // no --tier
        true, // force_ignore_tier_setting = true
        false,
    );
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
        msg.contains("Example: csa run --force-ignore-tier-setting"),
        "msg: {msg}"
    );

    // Missing only --thinking
    let result = super::resolve_tool_and_model(
        Some(ToolName::Codex), // --tool provided
        None,                  // no --model-spec
        Some("gpt-4"),         // --model provided
        None,                  // missing --thinking
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        None, // no --tier
        true, // force_ignore_tier_setting = true
        false,
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Missing required flags: --thinking"),
        "msg: {msg}"
    );

    // Missing only --model
    let result = super::resolve_tool_and_model(
        Some(ToolName::Codex), // --tool provided
        None,                  // no --model-spec
        None,                  // missing --model
        Some("high"),          // --thinking provided
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        None, // no --tier
        true, // force_ignore_tier_setting = true
        false,
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Missing required flags: --model"),
        "msg: {msg}"
    );

    // Missing only --tool
    let result = super::resolve_tool_and_model(
        None,          // missing --tool
        None,          // no --model-spec
        Some("gpt-4"), // --model provided
        Some("high"),  // --thinking provided
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        None, // no --tier
        true, // force_ignore_tier_setting = true
        false,
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("Missing required flags: --tool"), "msg: {msg}");
}

#[test]
fn resolve_tool_and_model_force_ignore_tier_allows_complete_spec() {
    let _guard = assume_tier_tools_available();
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-4/high"], &["codex"]);

    // All required flags provided - should succeed
    let result = super::resolve_tool_and_model(
        Some(ToolName::Codex), // --tool provided
        None,                  // no --model-spec
        Some("gpt-4"),         // --model provided
        Some("high"),          // --thinking provided
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        None, // no --tier
        true, // force_ignore_tier_setting = true
        false,
    );
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
fn resolve_tool_and_model_force_ignore_tier_bypassed_when_tier_provided() {
    let _guard = assume_tier_tools_available();
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-4/high"], &["codex"]);

    // When --tier is provided, validation should be skipped
    let result = super::resolve_tool_and_model(
        None, // no --tool
        None, // no --model-spec
        None, // no --model
        None, // no --thinking
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        Some("tier-1"), // --tier provided
        true,           // force_ignore_tier_setting = true
        false,
    );
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
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-4/high"], &["codex"]);

    // When --model-spec is provided, validation should be skipped
    let result = super::resolve_tool_and_model(
        None,                            // no --tool
        Some("codex/openai/gpt-4/high"), // --model-spec provided
        None,                            // no --model
        None,                            // no --thinking
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        None, // no --tier
        true, // force_ignore_tier_setting = true
        false,
    );
    // Should succeed because model_spec is provided, bypassing the validation
    assert!(
        result.is_ok(),
        "model_spec provided should bypass validation: {:?}",
        result
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

    // When no tiers are configured, validation should be skipped
    let result = super::resolve_tool_and_model(
        None, // no --tool
        None, // no --model-spec
        None, // no --model
        None, // no --thinking
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        None, // no --tier
        true, // force_ignore_tier_setting = true
        false,
    );
    // Should succeed because no tiers are configured, so validation is skipped
    assert!(
        result.is_ok(),
        "No tiers configured should skip validation: {:?}",
        result
    );
}

#[test]
fn resolve_tool_and_model_force_ignore_tier_skipped_when_flag_false() {
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-4/high"], &["codex"]);

    // When force_ignore_tier_setting = false, validation should be skipped
    let result = super::resolve_tool_and_model(
        None, // no --tool
        None, // no --model-spec
        None, // no --model
        None, // no --thinking
        Some(&cfg),
        std::path::Path::new("/tmp"),
        false,
        false,
        false,
        None,  // no --tier
        false, // force_ignore_tier_setting = false
        false,
    );
    // Should fail for different reason (tier enforcement), but not our new validation
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    // Should be the original tier enforcement error, not our new validation
    assert!(
        !msg.contains("When using --force-ignore-tier-setting"),
        "Should not trigger new validation when flag is false: {msg}"
    );
}
