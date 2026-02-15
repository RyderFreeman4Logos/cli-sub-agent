use anyhow::Result;
use std::path::Path;
use tracing::info;

use crate::cli::ClaudeSubAgentArgs;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{ToolArg, ToolName};

pub(crate) async fn handle_claude_sub_agent(
    args: ClaudeSubAgentArgs,
    current_depth: u32,
) -> Result<i32> {
    // 1. Determine project root
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;

    // 2. Load config and validate recursion depth
    let Some((config, global_config)) =
        crate::pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };

    // 3. Read prompt
    let prompt = crate::run_helpers::read_prompt(args.question)?;

    // 6. Resolve tool
    let parent_tool = crate::run_helpers::detect_parent_tool();
    let tool = resolve_claude_tool(
        args.tool,
        config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
        &project_root,
    )?;

    // 7. Resolve model
    let (tool_name, resolved_model_spec, resolved_model) =
        crate::run_helpers::resolve_tool_and_model(
            Some(tool),
            args.model_spec.as_deref(),
            args.model.as_deref(),
            config.as_ref(),
            &project_root,
        )?;

    // 8. Build executor and validate tool
    let executor = crate::pipeline::build_and_validate_executor(
        &tool_name,
        resolved_model_spec.as_deref(),
        resolved_model.as_deref(),
        None, // thinking budget
        config.as_ref(),
    )
    .await?;

    // 9. Acquire global slot to enforce concurrency limit
    let _slot_guard = crate::pipeline::acquire_slot(&executor, &global_config)?;

    // 10. Get env injection from global config
    let extra_env = global_config.env_vars(executor.tool_name());
    let idle_timeout_seconds = crate::pipeline::resolve_idle_timeout_seconds(config.as_ref(), None);

    // 11. Session description (no longer derived from --skill)
    let description: Option<String> = None;

    // 12. Execute with session
    let result = crate::pipeline::execute_with_session(
        &executor,
        &tool_name,
        &prompt,
        args.session,
        description,
        None, // parent
        &project_root,
        config.as_ref(),
        extra_env,
        Some("run"),
        None, // claude-sub-agent does not use tier-based selection
        None, // claude-sub-agent does not override context loading options
        csa_process::StreamMode::BufferOnly,
        idle_timeout_seconds,
        Some(&global_config),
    )
    .await?;

    // 13. Audit logging
    info!(
        tool = %tool_name.as_str(),
        exit_code = result.exit_code,
        "claude-sub-agent execution completed"
    );

    // 14. Print result
    print!("{}", result.output);

    Ok(result.exit_code)
}

/// Maximum SKILL.md file size (256 KB) to prevent excessive memory/token usage
/// Resolve which tool to use for claude-sub-agent.
/// Priority: CLI --tool > auto (heterogeneous selection based on parent tool,
/// then first enabled+available tool in preference order).
///
/// NOTE: `ToolArg::Auto` here intentionally uses "heterogeneous-preferred"
/// semantics (try hetero, fall back to any available) rather than the strict
/// semantics used by review/debate (which error when hetero is impossible).
/// This is because claude-sub-agent is a general-purpose executor where model
/// family diversity is preferred but not essential, unlike review/debate where
/// independent evaluation requires strict heterogeneity.
/// See: https://github.com/RyderFreeman4Logos/cli-sub-agent/issues/45
///
/// Config-based selection (`[claude-sub-agent].tool` in project/global config)
/// is not yet implemented — auto selection is the current default.
fn resolve_claude_tool(
    arg_tool: Option<ToolArg>,
    project_config: Option<&ProjectConfig>,
    _global_config: &GlobalConfig,
    parent_tool: Option<&str>,
    project_root: &Path,
) -> Result<ToolName> {
    // CLI override is highest priority
    if let Some(tool_arg) = arg_tool {
        return match tool_arg {
            ToolArg::Specific(t) => Ok(t),
            ToolArg::Auto => resolve_auto_tool(parent_tool, project_config, project_root),
            ToolArg::AnyAvailable => select_any_available_tool(project_config, project_root),
        };
    }

    // Fall through to auto selection
    resolve_auto_tool(parent_tool, project_config, project_root)
}

fn resolve_auto_tool(
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    project_root: &Path,
) -> Result<ToolName> {
    // Build the set of tools that are configured for auto selection and installed.
    let available_tools: Vec<ToolName> = get_auto_selectable_tools(project_config, project_root)
        .into_iter()
        .filter(|t| crate::run_helpers::is_tool_binary_available(t.as_str()))
        .collect();

    // Try heterogeneous selection on actually-available tools
    if let Some(parent) = parent_tool {
        if let Ok(parent_tool_name) = crate::run_helpers::parse_tool_name(parent) {
            if let Some(tool) = select_heterogeneous_tool(&parent_tool_name, &available_tools) {
                return Ok(tool);
            }
        }
    }

    // Fallback: first available in preference order
    for preferred in &["codex", "claude-code", "opencode", "gemini-cli"] {
        if let Ok(tool) = crate::run_helpers::parse_tool_name(preferred) {
            if available_tools.contains(&tool) {
                return Ok(tool);
            }
        }
    }

    anyhow::bail!("No suitable tool found for claude-sub-agent. Install codex or claude-code.")
}

/// Get tools eligible for auto/heterogeneous selection from project config.
fn get_auto_selectable_tools(
    project_config: Option<&ProjectConfig>,
    _project_root: &Path,
) -> Vec<ToolName> {
    if let Some(cfg) = project_config {
        csa_config::global::all_known_tools()
            .iter()
            .filter(|t| cfg.is_tool_auto_selectable(t.as_str()))
            .copied()
            .collect()
    } else {
        Vec::new()
    }
}

/// Select a heterogeneous tool based on parent tool
fn select_heterogeneous_tool(
    parent_tool: &ToolName,
    enabled_tools: &[ToolName],
) -> Option<ToolName> {
    csa_config::global::select_heterogeneous_tool(parent_tool, enabled_tools)
}

/// Select the first available enabled tool (in built-in preference order, no heterogeneity constraint)
fn select_any_available_tool(
    project_config: Option<&ProjectConfig>,
    project_root: &Path,
) -> Result<ToolName> {
    let enabled_tools = get_auto_selectable_tools(project_config, project_root);

    for tool in enabled_tools {
        if crate::run_helpers::is_tool_binary_available(tool.as_str()) {
            return Ok(tool);
        }
    }

    anyhow::bail!(
        "No tools available. Install at least one tool (codex, claude-code, opencode, gemini-cli)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use csa_config::{ProjectMeta, ResourcesConfig, TierConfig, ToolConfig};
    use std::collections::HashMap;

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
                },
            );
            tier_models.push(format!("{tool}/provider/model/medium"));
        }

        let mut tiers = HashMap::new();
        let mut tier_mapping = HashMap::new();
        if !tier_models.is_empty() {
            tiers.insert(
                "tier3".to_string(),
                TierConfig {
                    description: "test".to_string(),
                    models: tier_models,
                    token_budget: None,
                    max_turns: None,
                },
            );
            tier_mapping.insert("default".to_string(), "tier3".to_string());
        }

        ProjectConfig {
            schema_version: 1,
            project: ProjectMeta::default(),
            resources: ResourcesConfig::default(),
            tools: tool_map,
            review: None,
            debate: None,
            tiers,
            tier_mapping,
            aliases: HashMap::new(),
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
            },
        );
        tool_map.insert(
            "claude-code".to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
                suppress_notify: true,
            },
        );
        tool_map.insert(
            "gemini-cli".to_string(),
            ToolConfig {
                enabled: false,
                restrictions: None,
                suppress_notify: true,
            },
        );
        tool_map.insert(
            "opencode".to_string(),
            ToolConfig {
                enabled: false,
                restrictions: None,
                suppress_notify: true,
            },
        );

        let cfg = ProjectConfig {
            schema_version: 1,
            project: ProjectMeta::default(),
            resources: ResourcesConfig::default(),
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
                    token_budget: None,
                    max_turns: None,
                },
            )]),
            tier_mapping: HashMap::from([("default".to_string(), "tier3".to_string())]),
            aliases: HashMap::new(),
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
}
