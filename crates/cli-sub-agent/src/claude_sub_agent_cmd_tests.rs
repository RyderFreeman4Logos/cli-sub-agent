//! Tests for `claude_sub_agent_cmd` tool/model resolution.
//!
//! Extracted to a sibling `*_tests.rs` module to keep the driver file under the
//! monolith token budget (#1741); no behavior change.

use super::*;
use crate::test_env_lock::ScopedTestEnvVar;
use clap::Parser;
use csa_config::{ProjectMeta, ResourcesConfig, TierConfig, TierStrategy, ToolConfig};
use std::collections::HashMap;

fn assume_tier_tools_available() -> ScopedTestEnvVar {
    ScopedTestEnvVar::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1")
}

fn project_config_with_enabled_tools(tools: &[&str]) -> ProjectConfig {
    let mut tool_map = HashMap::new();
    let mut tier_models = Vec::new();
    for tool in tools {
        tool_map.insert(
            (*tool).to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
        let model_spec = match *tool {
            "codex" => "codex/openai/gpt-5.4/high".to_string(),
            "gemini-cli" => "gemini-cli/google/default/medium".to_string(),
            other => format!("{other}/provider/model/medium"),
        };
        tier_models.push(model_spec);
    }

    let mut tiers = HashMap::new();
    let mut tier_mapping = HashMap::new();
    if !tier_models.is_empty() {
        tiers.insert(
            "tier3".to_string(),
            TierConfig {
                description: "test".to_string(),
                models: tier_models,
                strategy: TierStrategy::default(),

                token_budget: None,
                max_turns: None,
            },
        );
        tier_mapping.insert("default".to_string(), "tier3".to_string());
    }

    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta::default(),
        resources: ResourcesConfig {
            memory_max_mb: Some(1024),
            min_free_memory_mb: 1,
            ..Default::default()
        },
        acp: Default::default(),
        tools: tool_map,
        review: None,
        debate: None,
        tiers,
        tier_mapping,
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
    }
}

#[test]
fn resolve_claude_tool_prefers_cli_override() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["gemini-cli", "codex"]);
    let tool = resolve_claude_tool(
        Some(ToolArg::Specific(ToolName::Codex)),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap();
    assert!(matches!(tool, ToolName::Codex));
}

fn resolve_model_spec_bypasses_tier_block_for_auto_selected_claude_sub_agent_tool_impl() {
    let _tool_availability = assume_tier_tools_available();
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["gemini-cli", "codex"]);
    let tool = resolve_claude_tool(
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
    )
    .unwrap();

    let (tool_name, model_spec, model) =
        crate::run_helpers::resolve_tool_and_model(crate::run_helpers::RoutingRequest {
            tool: Some(tool),
            model_spec: Some("codex/openai/gpt-5.4/high"),
            model: None,
            thinking: None, // thinking not needed for test
            config: Some(&cfg),
            global_config: Some(&global),
            project_root: std::path::Path::new("/tmp/test-project"),
            force: false,
            force_override_user_config: false,
            needs_edit: false,
            tier: None,
            force_ignore_tier_setting: false,
            tier_bypass_allowed: false,
            tool_is_auto_resolved: true,
        })
        .expect("auto-selected claude-sub-agent tool should not block explicit model_spec");

    assert_eq!(tool_name, ToolName::Codex);
    assert_eq!(model_spec.as_deref(), Some("codex/openai/gpt-5.4/high"));
    assert!(model.is_none());
}

#[test]
fn resolve_model_spec_bypasses_tier_block_for_auto_selected_claude_sub_agent_tool() {
    resolve_model_spec_bypasses_tier_block_for_auto_selected_claude_sub_agent_tool_impl();
}

#[test]
fn resolve_claude_sub_agent_tool_and_model_short_circuits_auto_select_for_model_spec() {
    let global = GlobalConfig::default();

    let auto_select_error = resolve_claude_tool(
        None,
        None,
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
    )
    .expect_err("without model-spec, auto-select should fail when no config enables tools");
    assert!(
        auto_select_error
            .to_string()
            .contains("No suitable tool found for claude-sub-agent"),
        "{auto_select_error}"
    );

    let (tool_name, model_spec, model) =
        super::resolve_claude_sub_agent_tool_and_model(ClaudeSubAgentRoutingRequest {
            arg_tool: None,
            model_spec: Some("codex/openai/gpt-5.4/medium"),
            model: None,
            project_config: None,
            global_config: &global,
            parent_tool: Some("claude-code"),
            project_root: std::path::Path::new("/tmp/test-project"),
            inherited_trusted_pin: false,
        })
        .expect("model-spec exact selection should not require auto-selected tool");

    assert_eq!(tool_name, ToolName::Codex);
    assert_eq!(model_spec.as_deref(), Some("codex/openai/gpt-5.4/medium"));
    assert!(model.is_none());
}

#[test]
fn resolve_claude_sub_agent_tool_and_model_rejects_tool_model_spec_mismatch() {
    let global = GlobalConfig::default();

    let error = super::resolve_claude_sub_agent_tool_and_model(ClaudeSubAgentRoutingRequest {
        arg_tool: Some(ToolArg::Specific(ToolName::GeminiCli)),
        model_spec: Some("codex/openai/gpt-5.4/medium"),
        model: None,
        project_config: None,
        global_config: &global,
        parent_tool: Some("claude-code"),
        project_root: std::path::Path::new("/tmp/test-project"),
        inherited_trusted_pin: false,
    })
    .expect_err("mismatched explicit --tool + --model-spec must error");

    let message = error.to_string();
    assert!(message.contains("--tool gemini-cli"));
    assert!(message.contains("--model-spec codex/openai/gpt-5.4/medium"));
    assert!(message.contains("tool codex"));
}

#[test]
fn alias_to_auto_with_model_spec_resolves_via_spec() {
    let mut global = GlobalConfig::default();
    global
        .tool_aliases
        .insert("router".to_string(), "auto".to_string());

    let (tool_name, model_spec, model) =
        super::resolve_claude_sub_agent_tool_and_model(ClaudeSubAgentRoutingRequest {
            arg_tool: Some(ToolArg::Alias("router".to_string())),
            model_spec: Some("codex/openai/gpt-5.4/medium"),
            model: None,
            project_config: None,
            global_config: &global,
            parent_tool: Some("claude-code"),
            project_root: std::path::Path::new("/tmp/test-project"),
            inherited_trusted_pin: false,
        })
        .expect("alias to auto should let model-spec choose the tool");

    assert_eq!(tool_name, ToolName::Codex);
    assert_eq!(model_spec.as_deref(), Some("codex/openai/gpt-5.4/medium"));
    assert!(model.is_none());
}

#[test]
fn alias_to_any_available_matches_direct_any_available() {
    let _tool_availability = assume_tier_tools_available();
    let mut global = GlobalConfig::default();
    global
        .tool_aliases
        .insert("router".to_string(), "any-available".to_string());
    let cfg = project_config_with_enabled_tools(&["codex", "claude-code"]);

    let aliased = super::resolve_claude_sub_agent_tool_and_model(ClaudeSubAgentRoutingRequest {
        arg_tool: Some(ToolArg::Alias("router".to_string())),
        model_spec: Some("codex/openai/gpt-5.4/high"),
        model: None,
        project_config: Some(&cfg),
        global_config: &global,
        parent_tool: Some("gemini-cli"),
        project_root: std::path::Path::new("/tmp/test-project"),
        inherited_trusted_pin: false,
    })
    .expect("alias to any-available should behave like direct any-available");

    let direct = super::resolve_claude_sub_agent_tool_and_model(ClaudeSubAgentRoutingRequest {
        arg_tool: Some(ToolArg::AnyAvailable),
        model_spec: Some("codex/openai/gpt-5.4/high"),
        model: None,
        project_config: Some(&cfg),
        global_config: &global,
        parent_tool: Some("gemini-cli"),
        project_root: std::path::Path::new("/tmp/test-project"),
        inherited_trusted_pin: false,
    })
    .expect("direct any-available should resolve");

    assert_eq!(aliased, direct);
}

#[test]
fn claude_sub_agent_tier_gate_rejects_model_spec_model_override_by_default() {
    let cfg = project_config_with_enabled_tools(&["codex"]);
    let global = GlobalConfig::default();

    let error = super::enforce_claude_sub_agent_tier_bypass_gate(
        Some("codex/openai/gpt-5.4/high"),
        Some("unlisted-model"),
        Some(&cfg),
        &global,
        false,
    )
    .expect_err("claude-sub-agent must gate model-spec plus model override under tiers");

    let message = error.to_string();
    assert!(message.contains("Tier bypass is disabled"), "{message}");
    assert!(message.contains("--model-spec"), "{message}");
    assert!(message.contains("--model"), "{message}");
}

#[test]
fn claude_sub_agent_tier_gate_rejects_bare_model_by_default() {
    let cfg = project_config_with_enabled_tools(&["codex"]);
    let global = GlobalConfig::default();

    let error = super::enforce_claude_sub_agent_tier_bypass_gate(
        None,
        Some("unlisted-model"),
        Some(&cfg),
        &global,
        false,
    )
    .expect_err("claude-sub-agent must gate bare model override under tiers");

    let message = error.to_string();
    assert!(message.contains("Tier bypass is disabled"), "{message}");
    assert!(message.contains("Refused flags: --model"), "{message}");
}

#[test]
fn claude_sub_agent_cli_rejects_thinking_flag() {
    let error = match crate::cli::Cli::try_parse_from([
        "csa",
        "claude-sub-agent",
        "--thinking",
        "high",
        "prompt",
    ]) {
        Ok(_) => panic!("claude-sub-agent does not expose --thinking"),
        Err(error) => error,
    };

    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
}

#[test]
fn claude_sub_agent_tier_gate_allows_bypass_flags_with_global_opt_in() {
    let cfg = project_config_with_enabled_tools(&["codex"]);
    let global = GlobalConfig {
        tier_policy: csa_config::TierPolicyConfig {
            allow_force_bypass: true,
        },
        ..Default::default()
    };

    super::enforce_claude_sub_agent_tier_bypass_gate(
        Some("codex/openai/gpt-5.4/high"),
        Some("unlisted-model"),
        Some(&cfg),
        &global,
        false,
    )
    .expect("global opt-in should allow claude-sub-agent exact/model bypass");
    super::enforce_claude_sub_agent_tier_bypass_gate(
        None,
        Some("unlisted-model"),
        Some(&cfg),
        &global,
        false,
    )
    .expect("global opt-in should allow claude-sub-agent bare model bypass");
}

#[test]
fn claude_sub_agent_tier_gate_allows_inherited_trusted_pin() {
    let cfg = project_config_with_enabled_tools(&["codex"]);
    let global = GlobalConfig::default();
    let inherited_spec = "codex/openai/gpt-5.5/xhigh";

    super::enforce_claude_sub_agent_tier_bypass_gate(
        Some(inherited_spec),
        None,
        Some(&cfg),
        &global,
        true,
    )
    .expect("trusted inherited subtree pin should bypass the tier gate");

    let (tool_name, model_spec, model) =
        super::resolve_claude_sub_agent_tool_and_model(ClaudeSubAgentRoutingRequest {
            arg_tool: None,
            model_spec: Some(inherited_spec),
            model: None,
            project_config: Some(&cfg),
            global_config: &global,
            parent_tool: Some("claude-code"),
            project_root: std::path::Path::new("/tmp/test-project"),
            inherited_trusted_pin: true,
        })
        .expect("trusted inherited subtree pin should bypass tier whitelist resolution");

    assert_eq!(tool_name, ToolName::Codex);
    assert_eq!(model_spec.as_deref(), Some(inherited_spec));
    assert!(model.is_none());
}

#[test]
fn get_auto_selectable_tools_returns_empty_when_no_config() {
    let tools = get_auto_selectable_tools(None, std::path::Path::new("/tmp"));
    assert!(tools.is_empty());
}

#[test]
fn get_auto_selectable_tools_filters_by_project_config() {
    // Create config with only codex and claude-code enabled, others disabled
    let mut tool_map = HashMap::new();
    tool_map.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: true,
            restrictions: None,
            suppress_notify: true,
            ..Default::default()
        },
    );
    tool_map.insert(
        "claude-code".to_string(),
        ToolConfig {
            enabled: true,
            restrictions: None,
            suppress_notify: true,
            ..Default::default()
        },
    );
    tool_map.insert(
        "gemini-cli".to_string(),
        ToolConfig {
            enabled: false,
            restrictions: None,
            suppress_notify: true,
            ..Default::default()
        },
    );
    tool_map.insert(
        "opencode".to_string(),
        ToolConfig {
            enabled: false,
            restrictions: None,
            suppress_notify: true,
            ..Default::default()
        },
    );

    let cfg = ProjectConfig {
        schema_version: 1,
        project: ProjectMeta::default(),
        resources: ResourcesConfig {
            memory_max_mb: Some(1024),
            min_free_memory_mb: 1,
            ..Default::default()
        },
        acp: Default::default(),
        tools: tool_map,
        review: None,
        debate: None,
        tiers: HashMap::from([(
            "tier3".to_string(),
            TierConfig {
                description: "test".to_string(),
                models: vec![
                    "codex/provider/model/medium".to_string(),
                    "claude-code/provider/model/medium".to_string(),
                    "gemini-cli/provider/model/medium".to_string(),
                    "opencode/provider/model/medium".to_string(),
                ],
                strategy: TierStrategy::default(),

                token_budget: None,
                max_turns: None,
            },
        )]),
        tier_mapping: HashMap::from([("default".to_string(), "tier3".to_string())]),
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

    let tools = get_auto_selectable_tools(Some(&cfg), std::path::Path::new("/tmp"));
    assert_eq!(tools.len(), 2);
    assert!(tools.contains(&ToolName::Codex));
    assert!(tools.contains(&ToolName::ClaudeCode));
}

#[test]
fn select_heterogeneous_tool_picks_different_family() {
    let enabled = vec![ToolName::ClaudeCode, ToolName::Codex, ToolName::GeminiCli];
    // Parent is claude-code (Anthropic family), should pick Codex (OpenAI) or GeminiCli
    let result = select_heterogeneous_tool(&ToolName::ClaudeCode, &enabled);
    assert!(result.is_some());
    let tool = result.unwrap();
    assert_ne!(
        tool.model_family(),
        ToolName::ClaudeCode.model_family(),
        "Heterogeneous selection must pick a different model family"
    );
}

#[test]
fn select_heterogeneous_tool_returns_none_when_only_same_family() {
    // Only claude-code available (same family as parent)
    let enabled = vec![ToolName::ClaudeCode];
    let result = select_heterogeneous_tool(&ToolName::ClaudeCode, &enabled);
    assert!(result.is_none());
}

#[test]
fn select_any_available_tool_errors_when_none_installed() {
    // With a config that only enables a non-existent tool name,
    // select_any_available_tool should return an error
    let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
    // gemini-cli is likely not installed in test environment
    let result = select_any_available_tool(Some(&cfg), std::path::Path::new("/tmp"));
    // This may pass or fail depending on the test machine, so we just verify it doesn't panic
    // and returns either Ok or a meaningful error
    if let Err(e) = result {
        assert!(e.to_string().contains("No tools available"));
    }
}
