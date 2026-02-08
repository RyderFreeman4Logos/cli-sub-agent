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

    // 4. Get git diff based on scope
    let diff_output = get_review_diff(&args)?;

    if diff_output.trim().is_empty() {
        eprintln!("No changes to review");
        return Ok(0);
    }

    // 5. Construct review prompt
    let prompt = construct_review_prompt(&args, &diff_output);

    // 6. Determine tool
    let tool = if let Some(t) = args.tool {
        t
    } else if let Some(ref cfg) = config {
        // Use first enabled tool from config
        cfg.tools
            .iter()
            .find(|(_, tool_cfg)| tool_cfg.enabled)
            .and_then(|(name, _)| match name.as_str() {
                "gemini-cli" => Some(ToolName::GeminiCli),
                "opencode" => Some(ToolName::Opencode),
                "codex" => Some(ToolName::Codex),
                "claude-code" => Some(ToolName::ClaudeCode),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("No enabled tools in project config"))?
    } else {
        // Default to gemini-cli
        ToolName::GeminiCli
    };

    // 7. Build executor
    let executor = crate::run_helpers::build_executor(&tool, None, args.model.as_deref(), None)?;

    // 8. Check tool is installed
    if let Err(e) = check_tool_installed(executor.executable_name()).await {
        error!(
            "Tool '{}' is not installed.\n\n{}\n\nOr disable it in .csa/config.toml:\n  [tools.{}]\n  enabled = false",
            executor.tool_name(),
            executor.install_hint(),
            executor.tool_name()
        );
        anyhow::bail!("{}", e);
    }

    // 9. Check tool is enabled in config
    if let Some(ref cfg) = config {
        if !cfg.is_tool_enabled(executor.tool_name()) {
            error!(
                "Tool '{}' is disabled in project config",
                executor.tool_name()
            );
            return Ok(1);
        }
    }

    // 10. Apply restrictions if configured
    let can_edit = config
        .as_ref()
        .map_or(true, |cfg| cfg.can_tool_edit_existing(executor.tool_name()));
    let effective_prompt = if !can_edit {
        info!(tool = %executor.tool_name(), "Applying edit restriction: tool cannot modify existing files");
        executor.apply_restrictions(&prompt, false)
    } else {
        prompt.clone()
    };

    // 11. Load global config for env injection and slot control
    let global_config = GlobalConfig::load()?;
    let extra_env = global_config.env_vars(executor.tool_name());

    // 11b. Acquire global slot to enforce concurrency limit
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

    // 12. Execute with session
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

    // 12. Print result
    print!("{}", result.output);

    Ok(result.exit_code)
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
