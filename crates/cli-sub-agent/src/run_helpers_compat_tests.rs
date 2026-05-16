use csa_core::types::ToolName;

use super::model_compat::{
    CODEX_CHATGPT_COMPATIBLE_MODELS, CODEX_CHATGPT_INCOMPATIBLE_MODELS, validate_tool_model_compat,
};

// --- unit tests for validate_tool_model_compat ---

#[test]
fn codex_rejects_o4_mini() {
    let err = validate_tool_model_compat(ToolName::Codex, "o4-mini", None).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("'o4-mini' is not supported"),
        "expected incompatibility message, got: {msg}"
    );
    assert!(
        msg.contains("ChatGPT account"),
        "expected ChatGPT mention: {msg}"
    );
    assert!(
        msg.contains("gpt-5.5"),
        "expected compatible model hint: {msg}"
    );
    assert!(
        msg.contains("default_model"),
        "expected suppression hint: {msg}"
    );
}

#[test]
fn codex_rejects_o4_mini_with_thinking_suffix() {
    // "o4-mini/high" has base model "o4-mini" — still incompatible.
    let err = validate_tool_model_compat(ToolName::Codex, "o4-mini/high", None).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("is not supported"),
        "thinking suffix should not bypass check: {msg}"
    );
}

#[test]
fn codex_accepts_o4_mini_high_as_distinct_model() {
    // "o4-mini-high" uses a hyphen, not a slash — it is a different model, not
    // "o4-mini" with a thinking budget.
    let result = validate_tool_model_compat(ToolName::Codex, "o4-mini-high", None);
    assert!(
        result.is_ok(),
        "o4-mini-high is a distinct model and should be accepted: {result:?}"
    );
}

#[test]
fn codex_accepts_all_known_compatible_models() {
    for &model in CODEX_CHATGPT_COMPATIBLE_MODELS {
        let result = validate_tool_model_compat(ToolName::Codex, model, None);
        assert!(
            result.is_ok(),
            "known-compatible model '{model}' was rejected: {result:?}"
        );
    }
}

#[test]
fn codex_bypasses_check_when_model_matches_configured_default() {
    // User explicitly configured o4-mini as default_model → accept it.
    let result = validate_tool_model_compat(ToolName::Codex, "o4-mini", Some("o4-mini"));
    assert!(
        result.is_ok(),
        "configured default_model should bypass compatibility check: {result:?}"
    );
}

#[test]
fn codex_bypasses_check_when_default_model_has_thinking_suffix() {
    // default_model = "o4-mini/high" → base is "o4-mini" → matches "o4-mini".
    let result = validate_tool_model_compat(ToolName::Codex, "o4-mini", Some("o4-mini/high"));
    assert!(
        result.is_ok(),
        "default_model with thinking suffix should bypass check: {result:?}"
    );
}

#[test]
fn codex_bypasses_check_when_model_has_suffix_and_default_does_not() {
    // model = "o4-mini/high", default_model = "o4-mini" → bases both "o4-mini" → bypass.
    let result = validate_tool_model_compat(ToolName::Codex, "o4-mini/high", Some("o4-mini"));
    assert!(
        result.is_ok(),
        "model with thinking suffix should match plain default_model: {result:?}"
    );
}

#[test]
fn non_codex_tools_accept_o4_mini_without_restriction() {
    for tool in [
        ToolName::ClaudeCode,
        ToolName::GeminiCli,
        ToolName::Opencode,
    ] {
        let result = validate_tool_model_compat(tool, "o4-mini", None);
        assert!(
            result.is_ok(),
            "tool {tool:?} should have no model restrictions: {result:?}"
        );
    }
}

#[test]
fn incompatible_models_list_is_non_empty() {
    assert!(
        !CODEX_CHATGPT_INCOMPATIBLE_MODELS.is_empty(),
        "incompatible models list must not be empty"
    );
}

#[test]
fn compatible_models_list_is_non_empty() {
    assert!(
        !CODEX_CHATGPT_COMPATIBLE_MODELS.is_empty(),
        "compatible models hint list must not be empty"
    );
}

// --- integration tests via resolve_tool_and_model ---

#[test]
fn resolve_tool_and_model_rejects_incompatible_codex_model() {
    // With no tiers configured, an explicit --tool codex --model o4-mini
    // should fail with a compatibility error before session spawn.
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        model: Some("o4-mini"),
        thinking: Some("medium"),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(result.is_err(), "should fail for incompatible model");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("is not supported"),
        "expected compat error: {msg}"
    );
}

#[test]
fn resolve_tool_and_model_rejects_incompatible_model_with_force_ignore_tier() {
    use super::tier_tests::config_with_tier;

    let _guard =
        crate::test_env_lock::ScopedTestEnvVar::set(super::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-5.5/high"], &["codex"]);

    // --force-ignore-tier-setting with incompatible model should still fail.
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        model: Some("o4-mini"),
        thinking: Some("medium"),
        config: Some(&cfg),
        force_ignore_tier_setting: true,
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(result.is_err(), "should fail for incompatible model");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("is not supported"),
        "expected compat error: {msg}"
    );
}

#[test]
fn resolve_tool_and_model_accepts_compatible_codex_model_with_force_ignore_tier() {
    use super::tier_tests::config_with_tier;

    let _guard =
        crate::test_env_lock::ScopedTestEnvVar::set(super::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let cfg = config_with_tier("tier-1", vec!["codex/openai/gpt-5.5/high"], &["codex"]);

    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        model: Some("gpt-5.5"),
        thinking: Some("high"),
        config: Some(&cfg),
        force_ignore_tier_setting: true,
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(result.is_ok(), "gpt-5.5 should be accepted: {result:?}");
    let (tool, _, model) = result.unwrap();
    assert_eq!(tool, ToolName::Codex);
    assert_eq!(model.as_deref(), Some("gpt-5.5"));
}

#[test]
fn resolve_tool_and_model_skips_compat_check_when_configured_default() {
    use csa_config::{ProjectConfig, ToolConfig};
    use std::collections::HashMap;

    let _guard =
        crate::test_env_lock::ScopedTestEnvVar::set(super::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");

    // Build a config where codex has default_model = "o4-mini".
    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: true,
            default_model: Some("o4-mini".to_string()),
            ..Default::default()
        },
    );
    let cfg = ProjectConfig {
        schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
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

    // Even though "o4-mini" is ordinarily incompatible, configured default bypasses the check.
    let result = super::resolve_tool_and_model(super::RoutingRequest {
        tool: Some(ToolName::Codex),
        model: Some("o4-mini"),
        thinking: Some("medium"),
        config: Some(&cfg),
        ..super::RoutingRequest::new(std::path::Path::new("/tmp"))
    });
    assert!(
        result.is_ok(),
        "configured default_model should bypass compat check: {result:?}"
    );
}
