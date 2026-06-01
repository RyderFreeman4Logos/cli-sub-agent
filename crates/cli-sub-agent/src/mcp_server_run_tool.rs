use anyhow::{Context, Result};
use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_executor::ResolvedTimeout;
use serde_json::Value;
use tempfile::TempDir;

/// Handle csa_run tool.
pub(super) async fn handle_run_tool(args: Value) -> Result<Value> {
    // Extract arguments
    let tool_str = args.get("tool").and_then(|v| v.as_str());
    let prompt = args
        .get("prompt")
        .and_then(|v| v.as_str())
        .context("Missing prompt argument")?;
    let session_arg = args
        .get("session")
        .and_then(|v| v.as_str())
        .map(String::from);
    let model_spec = args.get("model_spec").and_then(|v| v.as_str());
    let ephemeral = args
        .get("ephemeral")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let tier_arg = args.get("tier").and_then(|v| v.as_str());
    let force_ignore_tier = args
        .get("force_ignore_tier_setting")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Parse tool if provided
    let tool = if let Some(tool_str) = tool_str {
        Some(parse_tool_name(tool_str)?)
    } else {
        None
    };

    // Determine project root
    let project_root = crate::pipeline::determine_project_root(None)?;

    // Load config
    let config = ProjectConfig::load(&project_root)?;
    let global_config = csa_config::GlobalConfig::load()?;

    // Check recursion depth
    let current_depth: u32 = std::env::var("CSA_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let max_depth = config
        .as_ref()
        .map(|c| c.project.max_recursion_depth)
        .unwrap_or(5u32);

    if current_depth > max_depth {
        return Ok(serde_json::json!({
            "content": [
                {
                    "type": "text",
                    "text": format!(
                        "Error: Max recursion depth ({}) exceeded. Current: {}",
                        max_depth, current_depth
                    )
                }
            ]
        }));
    }

    crate::run_helpers::enforce_tier_bypass_gate(crate::run_helpers::TierBypassGateCtx {
        project_config: config.as_ref(),
        global_config: &global_config,
        model_spec: model_spec.is_some(),
        force: false,
        force_ignore_tier_setting: force_ignore_tier,
        model_tier_override: false,
        thinking_tier_override: false,
        inherited_trusted_pin: false,
    })?;

    // Resolve tool and model
    let (resolved_tool, resolved_model_spec, resolved_model) =
        crate::run_helpers::resolve_tool_and_model(crate::run_helpers::RoutingRequest {
            tool,
            model_spec,
            model: None,
            thinking: None, // MCP server does not support --thinking
            config: config.as_ref(),
            project_root: &project_root,
            force: false,                      // MCP server does not support --force
            force_override_user_config: false, // MCP server does not support --force-override-user-config
            needs_edit: false,                 // MCP tool dispatch always uses explicit tool
            tier: tier_arg,                    // --tier from MCP arguments
            force_ignore_tier_setting: force_ignore_tier, // --force-ignore-tier-setting from MCP arguments
            model_spec_tier_bypass_allowed: crate::run_helpers::tier_bypass_allowed(
                config.as_ref(),
                &global_config,
                false,
            ),
            tool_is_auto_resolved: false, // user-explicit tool from MCP args
        })?;

    // Build executor
    let executor = crate::run_helpers::build_executor(
        &resolved_tool,
        resolved_model_spec.as_deref(),
        resolved_model.as_deref(),
        None,
        config.as_ref(),
        false,
    )?;

    // Check tool is installed
    if csa_process::check_tool_installed(executor.runtime_binary_name())
        .await
        .is_err()
    {
        return Ok(serde_json::json!({
            "content": [
                {
                    "type": "text",
                    "text": format!(
                        "Error: Tool '{}' is not installed.\n\n{}\n\nOr disable it in .csa/config.toml",
                        executor.tool_name(),
                        executor.install_hint()
                    )
                }
            ]
        }));
    }

    // Check tool is enabled in config
    if let Some(ref cfg) = config
        && !cfg.is_tool_enabled(executor.tool_name())
    {
        return Ok(serde_json::json!({
            "content": [
                {
                    "type": "text",
                    "text": format!(
                        "Error: Tool '{}' is disabled in project config",
                        executor.tool_name()
                    )
                }
            ]
        }));
    }

    // Use global config for env injection and slot control
    let idle_timeout_seconds = crate::pipeline::resolve_idle_timeout_seconds(config.as_ref(), None);
    let initial_response_timeout_seconds =
        crate::pipeline::resolve_initial_response_timeout_for_tool(
            config.as_ref(),
            None,
            None,
            executor.tool_name(),
        );
    let extra_env = global_config.build_execution_env(
        executor.tool_name(),
        csa_config::ExecutionEnvOptions::default(),
    );
    let extra_env_ref = extra_env.as_ref();

    // Acquire global slot to enforce concurrency limit
    let max_concurrent = global_config.max_concurrent(executor.tool_name());
    let slots_dir = csa_config::GlobalConfig::slots_dir()?;
    let _slot_guard = match csa_lock::slot::try_acquire_slot(
        &slots_dir,
        executor.tool_name(),
        max_concurrent,
        None,
    ) {
        Ok(csa_lock::slot::SlotAcquireResult::Acquired(slot)) => Some(slot),
        Ok(csa_lock::slot::SlotAcquireResult::Exhausted(status)) => {
            return Ok(serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "Error: All {} slots for '{}' are occupied ({}/{}). Try again later.",
                        status.max_slots, executor.tool_name(), status.occupied, status.max_slots
                    )
                }]
            }));
        }
        Err(e) => {
            return Ok(serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": format!("Error: Slot acquisition failed: {}", e)
                }]
            }));
        }
    };

    // Execute
    let result = if ephemeral {
        // Ephemeral: use temp directory
        let temp_dir = TempDir::new()?;
        executor
            .execute_in(
                prompt,
                temp_dir.path(),
                extra_env_ref,
                // The MCP hub spawn path resolves no subtree pin (tier-routed
                // tool selection only); pin keys are stripped at
                // build_execution_env, so it never sets a pin (#1741).
                None,
                csa_process::StreamMode::BufferOnly,
                idle_timeout_seconds,
                direct_entry_resolved_timeout(initial_response_timeout_seconds),
            )
            .await?
    } else {
        // Persistent session
        crate::pipeline::execute_with_session(
            &executor,
            &resolved_tool,
            prompt,
            session_arg.clone(),
            false,
            None, // description
            None, // parent
            &project_root,
            config.as_ref(),
            extra_env_ref,
            None, // MCP hub resolves no subtree pin (#1741)
            Some("run"),
            None, // MCP server does not use tier-based selection
            None, // MCP server does not override context loading options
            csa_process::StreamMode::BufferOnly,
            idle_timeout_seconds,
            initial_response_timeout_seconds,
            None, // MCP server does not set wall-clock timeout
            None, // MCP server does not use memory injection
            Some(&global_config),
            None,  // MCP server does not run pre-session hooks
            false, // no_fs_sandbox
            false, // readonly_project_root
            &[],   // extra_writable
            &[],   // extra_readable
            false, // cli_no_error_marker_scan: no CLI flag here; defer to config (#1745)
        )
        .await?
    };

    // Format response
    let mut response_text = result.output.clone();

    // Add metadata section
    response_text.push_str("\n\n--- Execution Metadata ---\n");
    if !ephemeral {
        if let Some(ref sid) = session_arg {
            response_text.push_str(&format!("Session ID: {sid}\n"));
        } else {
            // For new sessions, we don't have the session ID here
            // since execute_with_session doesn't return it
            response_text.push_str("Session ID: (new session created)\n");
        }
    }
    response_text.push_str(&format!("Tool: {}\n", executor.tool_name()));
    response_text.push_str(&format!("Exit Code: {}\n", result.exit_code));
    if !result.summary.is_empty() {
        response_text.push_str(&format!("Summary: {}\n", result.summary));
    }
    if !result.stderr_output.trim().is_empty() {
        response_text.push_str("--- Stderr ---\n");
        response_text.push_str(result.stderr_output.trim());
        response_text.push('\n');
    }

    Ok(serde_json::json!({
        "content": [
            {
                "type": "text",
                "text": response_text
            }
        ]
    }))
}

/// Parse tool name from string.
pub(super) fn parse_tool_name(tool_str: &str) -> Result<ToolName> {
    match tool_str {
        "gemini-cli" => Ok(ToolName::GeminiCli),
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        "claude-code" => Ok(ToolName::ClaudeCode),
        "antigravity-cli" => Ok(ToolName::AntigravityCli),
        _ => anyhow::bail!("Unknown tool: {tool_str}"),
    }
}

pub(super) fn direct_entry_resolved_timeout(
    initial_response_timeout_seconds: Option<u64>,
) -> ResolvedTimeout {
    ResolvedTimeout(initial_response_timeout_seconds)
}
