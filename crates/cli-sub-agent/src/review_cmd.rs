use anyhow::{Context, Result};
use std::path::Path;
use tokio::task::JoinSet;
use tracing::{debug, info};

use crate::cli::ReviewArgs;
use crate::review_consensus::{
    CLEAN, agreement_level, build_multi_reviewer_instruction, build_reviewer_tools,
    consensus_strategy_label, consensus_verdict, parse_consensus_strategy, parse_review_verdict,
    resolve_consensus,
};
use csa_config::global::{heterogeneous_counterpart, select_heterogeneous_tool};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::consensus::AgentResponse;
use csa_core::types::ToolName;

#[derive(Debug, Clone)]
struct ReviewerOutcome {
    reviewer_index: usize,
    tool: ToolName,
    output: String,
    exit_code: i32,
    verdict: &'static str,
}

pub(crate) async fn handle_review(args: ReviewArgs, current_depth: u32) -> Result<i32> {
    // 1. Determine project root
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;

    // 2. Load config and validate recursion depth
    let Some((config, global_config)) =
        crate::pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };

    // 3. Derive scope and mode from CLI args
    let scope = derive_scope(&args);
    let mode = if args.fix {
        "review-and-fix"
    } else {
        "review-only"
    };

    debug!(scope = %scope, mode = %mode, security_mode = %args.security_mode, "Review parameters");

    // 4. Build review instruction (no diff content — tool loads skill and fetches diff itself)
    let prompt =
        build_review_instruction(&scope, mode, &args.security_mode, args.context.as_deref());

    // 5. Determine tool
    let detected_parent_tool = crate::run_helpers::detect_parent_tool();
    let parent_tool = crate::run_helpers::resolve_tool(detected_parent_tool, &global_config);
    let tool = resolve_review_tool(
        args.tool,
        config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
        &project_root,
    )?;

    if args.reviewers == 1 {
        // Keep single-reviewer behavior unchanged.
        let result = execute_review(
            tool,
            prompt,
            args.session,
            args.model,
            format!(
                "review: {}",
                crate::run_helpers::truncate_prompt(&scope, 80)
            ),
            &project_root,
            config.as_ref(),
            &global_config,
        )
        .await?;
        print!("{}", result.output);
        return Ok(result.exit_code);
    }

    if args.fix {
        anyhow::bail!("--fix is not supported when --reviewers > 1");
    }
    if args.session.is_some() {
        anyhow::bail!("--session is only supported when --reviewers=1");
    }

    let reviewers = args.reviewers as usize;
    let consensus_strategy = parse_consensus_strategy(&args.consensus)?;
    let reviewer_tools = build_reviewer_tools(
        args.tool,
        tool,
        config.as_ref(),
        Some(&global_config),
        reviewers,
    );

    let mut join_set = JoinSet::new();
    for (reviewer_index, reviewer_tool) in reviewer_tools.into_iter().enumerate() {
        let reviewer_prompt =
            build_multi_reviewer_instruction(&prompt, reviewer_index + 1, reviewer_tool);
        let reviewer_model = args.model.clone();
        let reviewer_project_root = project_root.clone();
        let reviewer_config = config.clone();
        let reviewer_global = global_config.clone();
        let reviewer_description = format!(
            "review[{}]: {}",
            reviewer_index + 1,
            crate::run_helpers::truncate_prompt(&scope, 80)
        );

        join_set.spawn(async move {
            let result = execute_review(
                reviewer_tool,
                reviewer_prompt,
                None,
                reviewer_model,
                reviewer_description,
                &reviewer_project_root,
                reviewer_config.as_ref(),
                &reviewer_global,
            )
            .await?;
            Ok::<ReviewerOutcome, anyhow::Error>(ReviewerOutcome {
                reviewer_index,
                tool: reviewer_tool,
                verdict: parse_review_verdict(&result.output, result.exit_code),
                output: result.output,
                exit_code: result.exit_code,
            })
        });
    }

    let mut outcomes = Vec::with_capacity(reviewers);
    while let Some(joined) = join_set.join_next().await {
        let outcome = joined.context("reviewer task join failure")??;
        outcomes.push(outcome);
    }
    outcomes.sort_by_key(|o| o.reviewer_index);

    let responses: Vec<AgentResponse> = outcomes
        .iter()
        .map(|o| AgentResponse {
            agent: format!("reviewer-{}:{}", o.reviewer_index + 1, o.tool.as_str()),
            content: o.verdict.to_string(),
            weight: 1.0,
            timed_out: false,
        })
        .collect();

    let consensus_result = resolve_consensus(consensus_strategy, &responses);
    let final_verdict = consensus_verdict(&consensus_result);
    let agreement = agreement_level(&consensus_result);

    for outcome in &outcomes {
        println!(
            "===== Reviewer {} ({}) | verdict={} | exit_code={} =====",
            outcome.reviewer_index + 1,
            outcome.tool,
            outcome.verdict,
            outcome.exit_code
        );
        print!("{}", outcome.output);
        if !outcome.output.ends_with('\n') {
            println!();
        }
    }

    println!("===== Consensus =====");
    println!(
        "strategy: {}",
        consensus_strategy_label(consensus_result.strategy_used)
    );
    println!("consensus_reached: {}", consensus_result.consensus_reached);
    println!("agreement_level: {:.0}%", agreement * 100.0);
    println!("final_decision: {final_verdict}");
    println!("individual_verdicts:");
    for outcome in &outcomes {
        println!(
            "- reviewer {} ({}) => {}",
            outcome.reviewer_index + 1,
            outcome.tool,
            outcome.verdict
        );
    }

    Ok(if final_verdict == CLEAN { 0 } else { 1 })
}

#[allow(clippy::too_many_arguments)]
async fn execute_review(
    tool: ToolName,
    prompt: String,
    session: Option<String>,
    model: Option<String>,
    description: String,
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Result<csa_process::ExecutionResult> {
    let executor = crate::pipeline::build_and_validate_executor(
        &tool,
        None,
        model.as_deref(),
        None,
        project_config,
        false, // skip tier whitelist for review tool selection
    )
    .await?;

    let can_edit =
        project_config.is_none_or(|cfg| cfg.can_tool_edit_existing(executor.tool_name()));
    let effective_prompt = if !can_edit {
        info!(tool = %executor.tool_name(), "Applying edit restriction: tool cannot modify existing files");
        executor.apply_restrictions(&prompt, false)
    } else {
        prompt
    };

    let extra_env = global_config.env_vars(executor.tool_name());
    let _slot_guard = crate::pipeline::acquire_slot(&executor, global_config)?;
    let idle_timeout_seconds = crate::pipeline::resolve_idle_timeout_seconds(project_config, None);

    crate::pipeline::execute_with_session(
        &executor,
        &tool,
        &effective_prompt,
        session,
        Some(description),
        None,
        project_root,
        project_config,
        extra_env,
        Some("review"),
        None,
        None,
        csa_process::StreamMode::BufferOnly,
        idle_timeout_seconds,
        Some(global_config),
    )
    .await
}

fn resolve_review_tool(
    arg_tool: Option<ToolName>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    parent_tool: Option<&str>,
    project_root: &Path,
) -> Result<ToolName> {
    if let Some(tool) = arg_tool {
        return Ok(tool);
    }

    if let Some(project_review) = project_config.and_then(|cfg| cfg.review.as_ref()) {
        return resolve_review_tool_from_value(
            &project_review.tool,
            parent_tool,
            project_config,
            global_config,
            project_root,
        )
        .with_context(|| {
            format!(
                "Failed to resolve review tool from project config: {}",
                ProjectConfig::config_path(project_root).display()
            )
        });
    }

    // When global [review].tool is "auto", try priority-aware selection first
    if global_config.review.tool == "auto" {
        let has_known_priority =
            csa_config::global::effective_tool_priority(project_config, global_config)
                .iter()
                .any(|entry| {
                    csa_config::global::all_known_tools()
                        .iter()
                        .any(|tool| tool.as_str() == entry)
                });
        if has_known_priority {
            if let Some(tool) = select_auto_review_tool(parent_tool, project_config, global_config)
            {
                return Ok(tool);
            }
        }
    }

    match global_config.resolve_review_tool(parent_tool) {
        Ok(tool_name) => crate::run_helpers::parse_tool_name(&tool_name).map_err(|_| {
            anyhow::anyhow!(
                "Invalid [review].tool value '{}'. Supported values: gemini-cli, opencode, codex, claude-code.",
                tool_name
            )
        }),
        Err(_) => Err(review_auto_resolution_error(parent_tool, project_root)),
    }
}

fn resolve_review_tool_from_value(
    tool_value: &str,
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    project_root: &Path,
) -> Result<ToolName> {
    if tool_value == "auto" {
        let has_known_priority =
            csa_config::global::effective_tool_priority(project_config, global_config)
                .iter()
                .any(|entry| {
                    csa_config::global::all_known_tools()
                        .iter()
                        .any(|tool| tool.as_str() == entry)
                });
        if has_known_priority {
            if let Some(tool) = select_auto_review_tool(parent_tool, project_config, global_config)
            {
                return Ok(tool);
            }
        }

        // Try old heterogeneous_counterpart first for backward compatibility
        if let Some(resolved) = parent_tool.and_then(heterogeneous_counterpart) {
            return crate::run_helpers::parse_tool_name(resolved).map_err(|_| {
                anyhow::anyhow!(
                    "BUG: auto review tool resolution returned invalid tool '{}'",
                    resolved
                )
            });
        }

        // Fallback to ModelFamily-based selection (filtered by enabled tools)
        if let Some(tool) = select_auto_review_tool(parent_tool, project_config, global_config) {
            return Ok(tool);
        }

        // Both methods failed
        return Err(review_auto_resolution_error(parent_tool, project_root));
    }

    crate::run_helpers::parse_tool_name(tool_value).map_err(|_| {
        anyhow::anyhow!(
            "Invalid project [review].tool value '{}'. Supported values: auto, gemini-cli, opencode, codex, claude-code.",
            tool_value
        )
    })
}

fn select_auto_review_tool(
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Option<ToolName> {
    let parent_str = parent_tool?;
    let parent_tool_name = crate::run_helpers::parse_tool_name(parent_str).ok()?;
    let enabled_tools: Vec<_> = if let Some(cfg) = project_config {
        let tools: Vec<_> = csa_config::global::all_known_tools()
            .iter()
            .filter(|t| cfg.is_tool_enabled(t.as_str()))
            .copied()
            .collect();
        csa_config::global::sort_tools_by_effective_priority(&tools, project_config, global_config)
    } else {
        csa_config::global::sort_tools_by_effective_priority(
            csa_config::global::all_known_tools(),
            project_config,
            global_config,
        )
    };

    select_heterogeneous_tool(&parent_tool_name, &enabled_tools)
}

fn review_auto_resolution_error(parent_tool: Option<&str>, project_root: &Path) -> anyhow::Error {
    let parent = parent_tool.unwrap_or("<none>").escape_default().to_string();
    let global_path = GlobalConfig::config_path()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.config/cli-sub-agent/config.toml".to_string());
    let project_path = ProjectConfig::config_path(project_root)
        .display()
        .to_string();

    anyhow::anyhow!(
        "AUTO review tool selection failed (tool = \"auto\").\n\n\
STOP: Do not proceed. Ask the user to configure the review tool explicitly.\n\n\
Parent tool context: {parent}\n\
Supported auto mapping: claude-code <-> codex\n\n\
Choose one:\n\
1) Global config (user-level): {global_path}\n\
   [review]\n\
   tool = \"codex\"  # or \"claude-code\", \"opencode\", \"gemini-cli\"\n\
2) Project config override: {project_path}\n\
   [review]\n\
   tool = \"codex\"  # or \"claude-code\", \"opencode\", \"gemini-cli\"\n\
3) CLI override: csa review --tool codex\n\n\
Reason: CSA enforces heterogeneity in auto mode and will not fall back."
    )
}

/// Derive the review scope string from CLI arguments.
///
/// Priority order (first match wins):
/// 1. `--range <from>...<to>` → "range:<from>...<to>"
/// 2. `--files <pathspec>`    → "files:<pathspec>"
/// 3. `--commit <sha>`        → "commit:<sha>"
/// 4. `--diff`                → "uncommitted"
/// 5. default                 → "base:<branch>" (branch defaults to "main")
fn derive_scope(args: &ReviewArgs) -> String {
    if let Some(ref range) = args.range {
        return format!("range:{range}");
    }
    if let Some(ref files) = args.files {
        return format!("files:{files}");
    }
    if let Some(ref commit) = args.commit {
        return format!("commit:{commit}");
    }
    if args.diff {
        return "uncommitted".to_string();
    }
    format!("base:{}", args.branch)
}

/// Build a concise review instruction that tells the tool to use the csa-review skill.
///
/// The tool loads the skill from `.claude/skills/csa-review/` automatically.
/// CSA only passes scope, mode, and optional parameters — no diff content.
fn build_review_instruction(
    scope: &str,
    mode: &str,
    security_mode: &str,
    context: Option<&str>,
) -> String {
    let mut instruction = format!(
        "Use the csa-review skill. scope={scope}, mode={mode}, security_mode={security_mode}."
    );
    if let Some(ctx) = context {
        instruction.push_str(&format!(" context={ctx}"));
    }
    instruction
}

#[cfg(test)]
#[path = "review_cmd_tests.rs"]
mod tests;
