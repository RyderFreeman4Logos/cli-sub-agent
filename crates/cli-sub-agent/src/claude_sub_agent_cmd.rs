use anyhow::{Result, anyhow};
use std::path::Path;
use tracing::info;

use crate::cli::ClaudeSubAgentArgs;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{ToolArg, ToolName};

pub(crate) async fn handle_claude_sub_agent(
    args: ClaudeSubAgentArgs,
    current_depth: u32,
) -> Result<i32> {
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;

    let Some((config, global_config)) =
        crate::pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };

    let prompt = crate::run_helpers::read_prompt(args.question)?;

    let parent_tool = crate::run_helpers::detect_parent_tool();
    let (tool_name, resolved_model_spec, resolved_model) = resolve_claude_sub_agent_tool_and_model(
        args.tool,
        args.model_spec.as_deref(),
        args.model.as_deref(),
        config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
        &project_root,
    )?;

    // 8. Build executor and validate tool
    let executor = crate::pipeline::build_and_validate_executor(
        &tool_name,
        resolved_model_spec.as_deref(),
        resolved_model.as_deref(),
        None, // thinking budget
        crate::pipeline::ConfigRefs {
            project: config.as_ref(),
            global: Some(&global_config),
        },
        args.model_spec.is_none(),
        false, // claude-sub-agent does not support --force-override-user-config
        false, // scoped to `csa run --tool`, not sub-agent orchestration
    )
    .await?;

    let _slot_guard = crate::pipeline::acquire_slot(&executor, &global_config)?;

    let extra_env = global_config.build_execution_env(
        executor.tool_name(),
        csa_config::ExecutionEnvOptions::default(),
    );
    // #1741: claude-sub-agent selects its own tool/model (incl. an explicit
    // --model-spec for THIS spawn) and does NOT consume the parent's subtree pin
    // for that choice, but it MUST still cascade an inherited pin so nested CSA
    // calls stay pinned. The pin is carried out-of-band as a typed value (None
    // unless this process is a pinned child) and applied by the executor's
    // trusted channel — never via the env map. The explicit --model-spec governs
    // only this spawn, not the pin.
    let subtree_pin = crate::run_cmd_model_pin::inherited_subtree_model_pin();
    let extra_env_ref = extra_env.as_ref();
    let idle_timeout_seconds = crate::pipeline::resolve_idle_timeout_seconds(config.as_ref(), None);
    let initial_response_timeout_seconds =
        crate::pipeline::resolve_initial_response_timeout_for_tool(
            config.as_ref(),
            None,
            None,
            executor.tool_name(),
        );

    let description: Option<String> = None;

    let result = crate::pipeline::execute_with_session(
        &executor,
        &tool_name,
        &prompt,
        args.session,
        false,
        description,
        None, // parent
        &project_root,
        config.as_ref(),
        extra_env_ref,
        subtree_pin.as_ref(),
        Some("run"),
        None, // claude-sub-agent does not use tier-based selection
        None, // claude-sub-agent does not override context loading options
        csa_process::StreamMode::BufferOnly,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        None, // claude-sub-agent does not set wall-clock timeout
        None, // claude-sub-agent does not use memory injection
        Some(&global_config),
        None,  // claude-sub-agent does not run pre-session hooks
        false, // no_fs_sandbox
        false, // readonly_project_root
        &[],   // extra_writable
        &[],   // extra_readable
        false, // cli_no_error_marker_scan: no CLI flag here; defer to config (#1745)
    )
    .await?;

    info!(
        tool = %tool_name.as_str(),
        exit_code = result.exit_code,
        "claude-sub-agent execution completed"
    );

    print!("{}", result.output);
    if !result.summary.trim().is_empty() {
        eprintln!("summary: {}", result.summary);
    }
    if !result.stderr_output.trim().is_empty() {
        eprintln!("{}", result.stderr_output.trim());
    }

    Ok(result.exit_code)
}

fn resolve_claude_sub_agent_tool_and_model(
    arg_tool: Option<ToolArg>,
    model_spec: Option<&str>,
    model: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    parent_tool: Option<&str>,
    project_root: &Path,
) -> Result<(ToolName, Option<String>, Option<String>)> {
    let resolved_arg_tool =
        resolve_tool_arg_alias(arg_tool, project_config, global_config).map_err(|e| anyhow!(e))?;
    let user_explicit_tool = matches!(resolved_arg_tool, Some(ToolArg::Specific(_)));
    let resolved_tool = if model_spec.is_some() && !user_explicit_tool {
        None
    } else {
        Some(resolve_claude_tool(
            resolved_arg_tool,
            project_config,
            global_config,
            parent_tool,
            project_root,
        )?)
    };

    crate::run_helpers::resolve_tool_and_model(crate::run_helpers::RoutingRequest {
        tool: resolved_tool,
        model_spec,
        model,
        thinking: None, // claude-sub-agent does not support --thinking
        config: project_config,
        project_root,
        force: false,                      // claude-sub-agent does not support --force
        force_override_user_config: false, // claude-sub-agent does not support --force-override-user-config
        needs_edit: false, // claude-sub-agent does not require edit-capable tool filtering
        tier: None,        // claude-sub-agent does not support --tier
        force_ignore_tier_setting: false, // claude-sub-agent does not support --force-ignore-tier-setting
        tool_is_auto_resolved: !user_explicit_tool, // treat auto/implicit selection as non-explicit
    })
}

fn resolve_tool_arg_alias(
    arg_tool: Option<ToolArg>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> std::result::Result<Option<ToolArg>, String> {
    let mut merged_aliases = global_config.tool_aliases.clone();
    if let Some(c) = project_config {
        merged_aliases.extend(c.tool_aliases.iter().map(|(k, v)| (k.clone(), v.clone())));
    }

    arg_tool
        .map(|tool_arg| tool_arg.resolve_alias(&merged_aliases))
        .transpose()
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
    global_config: &GlobalConfig,
    parent_tool: Option<&str>,
    project_root: &Path,
) -> Result<ToolName> {
    // CLI override is highest priority
    if let Some(tool_arg) = arg_tool {
        let resolved = resolve_tool_arg_alias(Some(tool_arg), project_config, global_config)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .expect("Some(tool_arg) should remain Some after alias resolution");
        return match resolved {
            ToolArg::Specific(t) => Ok(t),
            ToolArg::Auto => resolve_auto_tool(parent_tool, project_config, project_root),
            ToolArg::AnyAvailable => select_any_available_tool(project_config, project_root),
            ToolArg::Alias(_) => unreachable!("resolve_alias eliminates Alias variant"),
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
        .filter(|t| {
            crate::run_helpers::is_tool_binary_available_for_config(t.as_str(), project_config)
        })
        .collect();

    // Try heterogeneous selection on actually-available tools
    if let Some(parent) = parent_tool
        && let Ok(parent_tool_name) = crate::run_helpers::parse_tool_name(parent)
        && let Some(tool) = select_heterogeneous_tool(&parent_tool_name, &available_tools)
    {
        return Ok(tool);
    }

    // Fallback: first available in preference order
    for preferred in &["codex", "claude-code", "opencode", "gemini-cli"] {
        if let Ok(tool) = crate::run_helpers::parse_tool_name(preferred)
            && available_tools.contains(&tool)
        {
            return Ok(tool);
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
        if crate::run_helpers::is_tool_binary_available_for_config(tool.as_str(), project_config) {
            return Ok(tool);
        }
    }

    anyhow::bail!(
        "No tools available. Install at least one tool (codex, claude-code, opencode, gemini-cli)."
    )
}

#[cfg(test)]
#[path = "claude_sub_agent_cmd_tests.rs"]
mod tests;
