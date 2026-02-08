use anyhow::{Context, Result};
use tracing::{error, info};

use crate::cli::ReviewArgs;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_process::check_tool_installed;

pub(crate) async fn handle_review(args: ReviewArgs, current_depth: u32) -> Result<i32> {
    // 1. Determine project root
    let project_root = crate::determine_project_root(args.cd.as_deref())?;

    // 2. Load config (optional)
    let config = ProjectConfig::load(&project_root)?;

    // 3. Check recursion depth (from config or default)
    let max_depth = config
        .as_ref()
        .map(|c| c.project.max_recursion_depth)
        .unwrap_or(5u32);
    if current_depth > max_depth {
        error!(
            "Max recursion depth ({}) exceeded. Current: {}. Do it yourself.",
            max_depth, current_depth
        );
        return Ok(1);
    }

    // 4. Load global config for review tool selection + env injection + slot control.
    let global_config = GlobalConfig::load()?;

    // 5. Get git diff based on scope
    let diff_output = get_review_diff(&args)?;

    if diff_output.trim().is_empty() {
        eprintln!("No changes to review");
        return Ok(0);
    }

    // 6. Construct review prompt
    let prompt = construct_review_prompt(&args, &diff_output);

    // 7. Determine tool
    let parent_tool = std::env::var("CSA_TOOL")
        .ok()
        .or_else(|| std::env::var("CSA_PARENT_TOOL").ok());
    let tool = resolve_review_tool(
        args.tool,
        config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
    )?;

    // 8. Build executor
    let executor = crate::run_helpers::build_executor(
        &tool,
        None,
        args.model.as_deref(),
        None,
        config.as_ref(),
    )?;

    // 9. Check tool is installed
    if let Err(e) = check_tool_installed(executor.executable_name()).await {
        error!(
            "Tool '{}' is not installed.\n\n{}\n\nOr disable it in .csa/config.toml:\n  [tools.{}]\n  enabled = false",
            executor.tool_name(),
            executor.install_hint(),
            executor.tool_name()
        );
        anyhow::bail!("{}", e);
    }

    // 10. Check tool is enabled in config
    if let Some(ref cfg) = config {
        if !cfg.is_tool_enabled(executor.tool_name()) {
            error!(
                "Tool '{}' is disabled in project config",
                executor.tool_name()
            );
            return Ok(1);
        }
    }

    // 11. Apply restrictions if configured
    let can_edit = config
        .as_ref()
        .map_or(true, |cfg| cfg.can_tool_edit_existing(executor.tool_name()));
    let effective_prompt = if !can_edit {
        info!(tool = %executor.tool_name(), "Applying edit restriction: tool cannot modify existing files");
        executor.apply_restrictions(&prompt, false)
    } else {
        prompt.clone()
    };

    // 12. Get env injection from global config
    let extra_env = global_config.env_vars(executor.tool_name());

    // 12b. Acquire global slot to enforce concurrency limit
    let max_concurrent = global_config.max_concurrent(executor.tool_name());
    let slots_dir = GlobalConfig::slots_dir()?;
    let _slot_guard = match csa_lock::slot::try_acquire_slot(
        &slots_dir,
        executor.tool_name(),
        max_concurrent,
        None,
    ) {
        Ok(csa_lock::slot::SlotAcquireResult::Acquired(slot)) => slot,
        Ok(csa_lock::slot::SlotAcquireResult::Exhausted(status)) => {
            anyhow::bail!(
                "All {} slots for '{}' occupied ({}/{}). Try again later or use --tool to switch.",
                max_concurrent,
                executor.tool_name(),
                status.occupied,
                status.max_slots,
            );
        }
        Err(e) => {
            anyhow::bail!(
                "Slot acquisition failed for '{}': {}",
                executor.tool_name(),
                e
            );
        }
    };

    // 13. Execute with session
    let result = crate::execute_with_session(
        &executor,
        &tool,
        &effective_prompt,
        args.session,
        Some("Code review session".to_string()),
        None,
        &project_root,
        config.as_ref(),
        extra_env,
    )
    .await?;

    // 14. Print result
    print!("{}", result.output);

    Ok(result.exit_code)
}

fn resolve_review_tool(
    arg_tool: Option<ToolName>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    parent_tool: Option<&str>,
) -> Result<ToolName> {
    if let Some(tool) = arg_tool {
        return Ok(tool);
    }

    match global_config.resolve_review_tool(parent_tool) {
        Ok(tool_name) => parse_tool_name(&tool_name).ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid [review].tool value '{}'. Supported values: gemini-cli, opencode, codex, claude-code.",
                tool_name
            )
        }),
        Err(err) => {
            // Keep `csa review` usable in top-level/manual flows where no parent tool context exists.
            // In these cases we preserve the historical fallback behavior.
            info!(
                reason = %err,
                "Review tool auto-detection unavailable, falling back to project/default tool selection"
            );
            fallback_review_tool(project_config)
        }
    }
}

fn fallback_review_tool(project_config: Option<&ProjectConfig>) -> Result<ToolName> {
    if let Some(cfg) = project_config {
        cfg.tools
            .iter()
            .find(|(_, tool_cfg)| tool_cfg.enabled)
            .and_then(|(name, _)| parse_tool_name(name))
            .ok_or_else(|| anyhow::anyhow!("No enabled tools in project config"))
    } else {
        Ok(ToolName::GeminiCli)
    }
}

fn parse_tool_name(name: &str) -> Option<ToolName> {
    match name {
        "gemini-cli" => Some(ToolName::GeminiCli),
        "opencode" => Some(ToolName::Opencode),
        "codex" => Some(ToolName::Codex),
        "claude-code" => Some(ToolName::ClaudeCode),
        _ => None,
    }
}

pub(crate) fn get_review_diff(args: &ReviewArgs) -> Result<String> {
    let output = if let Some(ref commit) = args.commit {
        // Review specific commit
        std::process::Command::new("git")
            .arg("show")
            .arg(commit)
            .output()
            .with_context(|| format!("Failed to run git show for commit: {}", commit))?
    } else if args.diff {
        // Review uncommitted changes
        std::process::Command::new("git")
            .arg("diff")
            .arg("HEAD")
            .output()
            .context("Failed to run git diff")?
    } else {
        // Compare against branch (default: main)
        let branch = &args.branch;
        std::process::Command::new("git")
            .arg("diff")
            .arg(format!("{}...HEAD", branch))
            .output()
            .with_context(|| format!("Failed to run git diff against branch: {}", branch))?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Check for specific error patterns and provide friendly messages
        if stderr.contains("unknown revision") || stderr.contains("ambiguous argument") {
            if let Some(ref commit) = args.commit {
                anyhow::bail!(
                    "Commit '{}' not found. Ensure the commit SHA exists.",
                    commit
                );
            } else if !args.diff {
                anyhow::bail!(
                    "Branch '{}' not found. Ensure the branch exists locally.",
                    args.branch
                );
            }
        }

        anyhow::bail!("Git command failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub(crate) fn construct_review_prompt(args: &ReviewArgs, diff: &str) -> String {
    let default_instruction = "Review the following code changes for bugs, security issues, and code quality. Provide specific, actionable feedback.";

    let instruction = if let Some(ref custom_prompt) = args.prompt {
        format!("{}\n\n{}", default_instruction, custom_prompt)
    } else {
        default_instruction.to_string()
    };

    format!("{}\n\n```diff\n{}\n```", instruction, diff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig};
    use std::collections::HashMap;

    fn project_config_with_enabled_tools(tools: &[&str]) -> ProjectConfig {
        let mut tool_map = HashMap::new();
        for tool in tools {
            tool_map.insert(
                (*tool).to_string(),
                ToolConfig {
                    enabled: true,
                    restrictions: None,
                    suppress_notify: false,
                },
            );
        }

        ProjectConfig {
            schema_version: 1,
            project: ProjectMeta::default(),
            resources: ResourcesConfig::default(),
            tools: tool_map,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    #[test]
    fn resolve_review_tool_prefers_cli_override() {
        let global = GlobalConfig::default();
        let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
        let tool = resolve_review_tool(
            Some(ToolName::Codex),
            Some(&cfg),
            &global,
            Some("claude-code"),
        )
        .unwrap();
        assert!(matches!(tool, ToolName::Codex));
    }

    #[test]
    fn resolve_review_tool_uses_global_review_config_with_parent_tool() {
        let global = GlobalConfig::default();
        let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
        let tool = resolve_review_tool(None, Some(&cfg), &global, Some("claude-code")).unwrap();
        assert!(matches!(tool, ToolName::Codex));
    }

    #[test]
    fn resolve_review_tool_falls_back_without_parent_tool_context() {
        let global = GlobalConfig::default();
        let cfg = project_config_with_enabled_tools(&["opencode"]);
        let tool = resolve_review_tool(None, Some(&cfg), &global, None).unwrap();
        assert!(matches!(tool, ToolName::Opencode));
    }

    #[test]
    fn resolve_review_tool_errors_on_invalid_explicit_global_tool() {
        let mut global = GlobalConfig::default();
        global.review.tool = "invalid-tool".to_string();
        let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
        let err = resolve_review_tool(None, Some(&cfg), &global, Some("codex")).unwrap_err();
        assert!(err
            .to_string()
            .contains("Invalid [review].tool value 'invalid-tool'"));
    }
}
