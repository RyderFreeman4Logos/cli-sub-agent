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
    let reviewer_tools = build_reviewer_tools(args.tool, tool, config.as_ref(), reviewers);

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
            project_root,
        )
        .with_context(|| {
            format!(
                "Failed to resolve review tool from project config: {}",
                ProjectConfig::config_path(project_root).display()
            )
        });
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
    project_root: &Path,
) -> Result<ToolName> {
    if tool_value == "auto" {
        // Try old heterogeneous_counterpart first for backward compatibility
        if let Some(resolved) = parent_tool.and_then(heterogeneous_counterpart) {
            return crate::run_helpers::parse_tool_name(resolved).map_err(|_| {
                anyhow::anyhow!(
                    "BUG: auto review tool resolution returned invalid tool '{}'",
                    resolved
                )
            });
        }

        // Fallback to new ModelFamily-based selection (filtered by enabled tools)
        if let Some(parent_str) = parent_tool {
            if let Ok(parent_tool_name) = crate::run_helpers::parse_tool_name(parent_str) {
                let enabled_tools: Vec<_> = if let Some(cfg) = project_config {
                    csa_config::global::all_known_tools()
                        .iter()
                        .filter(|t| cfg.is_tool_enabled(t.as_str()))
                        .copied()
                        .collect()
                } else {
                    csa_config::global::all_known_tools().to_vec()
                };
                if let Some(tool) = select_heterogeneous_tool(&parent_tool_name, &enabled_tools) {
                    return Ok(tool);
                }
            }
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
mod tests {
    use super::*;
    use crate::cli::{Cli, Commands};
    use clap::Parser;
    use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig};
    use std::collections::HashMap;

    fn project_config_with_enabled_tools(tools: &[&str]) -> ProjectConfig {
        let mut tool_map = HashMap::new();
        for tool in csa_config::global::all_known_tools() {
            tool_map.insert(
                tool.as_str().to_string(),
                ToolConfig {
                    enabled: false,
                    restrictions: None,
                    suppress_notify: true,
                },
            );
        }
        for tool in tools {
            tool_map.insert(
                (*tool).to_string(),
                ToolConfig {
                    enabled: true,
                    restrictions: None,
                    suppress_notify: true,
                },
            );
        }

        ProjectConfig {
            schema_version: 1,
            project: ProjectMeta::default(),
            resources: ResourcesConfig::default(),
            tools: tool_map,
            review: None,
            debate: None,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    fn parse_review_args(argv: &[&str]) -> ReviewArgs {
        let cli = Cli::try_parse_from(argv).expect("review CLI args should parse");
        match cli.command {
            Commands::Review(args) => args,
            _ => panic!("expected review subcommand"),
        }
    }

    // --- resolve_review_tool tests ---

    #[test]
    fn resolve_review_tool_prefers_cli_override() {
        let global = GlobalConfig::default();
        let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
        let tool = resolve_review_tool(
            Some(ToolName::Codex),
            Some(&cfg),
            &global,
            Some("claude-code"),
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap();
        assert!(matches!(tool, ToolName::Codex));
    }

    #[test]
    fn resolve_review_tool_uses_global_review_config_with_parent_tool() {
        let global = GlobalConfig::default();
        let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
        let tool = resolve_review_tool(
            None,
            Some(&cfg),
            &global,
            Some("claude-code"),
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap();
        assert!(matches!(tool, ToolName::Codex));
    }

    #[test]
    fn resolve_review_tool_errors_without_parent_tool_context() {
        let global = GlobalConfig::default();
        let cfg = project_config_with_enabled_tools(&["opencode"]);
        let err = resolve_review_tool(
            None,
            Some(&cfg),
            &global,
            None,
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("AUTO review tool selection failed")
        );
    }

    #[test]
    fn resolve_review_tool_errors_on_invalid_explicit_global_tool() {
        let mut global = GlobalConfig::default();
        global.review.tool = "invalid-tool".to_string();
        let cfg = project_config_with_enabled_tools(&["gemini-cli"]);
        let err = resolve_review_tool(
            None,
            Some(&cfg),
            &global,
            Some("codex"),
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("Invalid [review].tool value 'invalid-tool'")
        );
    }

    #[test]
    fn resolve_review_tool_prefers_project_override() {
        let global = GlobalConfig::default();
        let mut cfg = project_config_with_enabled_tools(&["codex", "opencode"]);
        cfg.review = Some(csa_config::global::ReviewConfig {
            tool: "opencode".to_string(),
        });
        cfg.debate = None;

        let tool = resolve_review_tool(
            None,
            Some(&cfg),
            &global,
            Some("claude-code"),
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap();
        assert!(matches!(tool, ToolName::Opencode));
    }

    #[test]
    fn resolve_review_tool_project_auto_maps_to_heterogeneous_counterpart() {
        let global = GlobalConfig::default();
        let mut cfg = project_config_with_enabled_tools(&["codex", "claude-code"]);
        cfg.review = Some(csa_config::global::ReviewConfig {
            tool: "auto".to_string(),
        });
        cfg.debate = None;

        let tool = resolve_review_tool(
            None,
            Some(&cfg),
            &global,
            Some("claude-code"),
            std::path::Path::new("/tmp/test-project"),
        )
        .unwrap();
        assert!(matches!(tool, ToolName::Codex));
    }

    // --- derive_scope tests ---

    #[test]
    fn derive_scope_uncommitted() {
        let args = ReviewArgs {
            tool: None,
            session: None,
            model: None,
            diff: true,
            branch: "main".to_string(),
            commit: None,
            range: None,
            files: None,
            fix: false,
            security_mode: "auto".to_string(),
            context: None,
            reviewers: 1,
            consensus: "majority".to_string(),
            cd: None,
        };
        assert_eq!(derive_scope(&args), "uncommitted");
    }

    #[test]
    fn derive_scope_commit() {
        let args = ReviewArgs {
            tool: None,
            session: None,
            model: None,
            diff: false,
            branch: "main".to_string(),
            commit: Some("abc123".to_string()),
            range: None,
            files: None,
            fix: false,
            security_mode: "auto".to_string(),
            context: None,
            reviewers: 1,
            consensus: "majority".to_string(),
            cd: None,
        };
        assert_eq!(derive_scope(&args), "commit:abc123");
    }

    #[test]
    fn derive_scope_range() {
        let args = ReviewArgs {
            tool: None,
            session: None,
            model: None,
            diff: false,
            branch: "main".to_string(),
            commit: None,
            range: Some("main...HEAD".to_string()),
            files: None,
            fix: false,
            security_mode: "auto".to_string(),
            context: None,
            reviewers: 1,
            consensus: "majority".to_string(),
            cd: None,
        };
        assert_eq!(derive_scope(&args), "range:main...HEAD");
    }

    #[test]
    fn derive_scope_files() {
        let args = ReviewArgs {
            tool: None,
            session: None,
            model: None,
            diff: false,
            branch: "main".to_string(),
            commit: None,
            range: None,
            files: Some("src/**/*.rs".to_string()),
            fix: false,
            security_mode: "auto".to_string(),
            context: None,
            reviewers: 1,
            consensus: "majority".to_string(),
            cd: None,
        };
        assert_eq!(derive_scope(&args), "files:src/**/*.rs");
    }

    #[test]
    fn derive_scope_default_branch() {
        let args = ReviewArgs {
            tool: None,
            session: None,
            model: None,
            diff: false,
            branch: "develop".to_string(),
            commit: None,
            range: None,
            files: None,
            fix: false,
            security_mode: "auto".to_string(),
            context: None,
            reviewers: 1,
            consensus: "majority".to_string(),
            cd: None,
        };
        assert_eq!(derive_scope(&args), "base:develop");
    }

    #[test]
    fn derive_scope_range_takes_priority_over_commit() {
        let args = ReviewArgs {
            tool: None,
            session: None,
            model: None,
            diff: true,
            branch: "main".to_string(),
            commit: Some("abc".to_string()),
            range: Some("v1...v2".to_string()),
            files: None,
            fix: false,
            security_mode: "auto".to_string(),
            context: None,
            reviewers: 1,
            consensus: "majority".to_string(),
            cd: None,
        };
        // --range has highest priority
        assert_eq!(derive_scope(&args), "range:v1...v2");
    }

    #[test]
    fn review_cli_parses_range_scope_with_multiple_reviewers() {
        let args = parse_review_args(&[
            "csa",
            "review",
            "--range",
            "main...HEAD",
            "--reviewers",
            "3",
        ]);

        assert_eq!(args.reviewers, 3);
        assert_eq!(derive_scope(&args), "range:main...HEAD");
    }

    #[test]
    fn review_cli_parses_weighted_consensus_for_multi_reviewer_mode() {
        let args = parse_review_args(&[
            "csa",
            "review",
            "--diff",
            "--reviewers",
            "2",
            "--consensus",
            "weighted",
        ]);

        let strategy = parse_consensus_strategy(&args.consensus).unwrap();
        assert_eq!(consensus_strategy_label(strategy), "weighted");
    }

    #[test]
    fn review_cli_builds_multi_reviewer_config_from_args() {
        let args = parse_review_args(&[
            "csa",
            "review",
            "--tool",
            "codex",
            "--reviewers",
            "4",
            "--consensus",
            "unanimous",
        ]);

        let strategy = parse_consensus_strategy(&args.consensus).unwrap();
        let reviewers = args.reviewers as usize;
        let reviewer_tools = build_reviewer_tools(args.tool, ToolName::Codex, None, reviewers);

        assert!(reviewers > 1);
        assert_eq!(consensus_strategy_label(strategy), "unanimous");
        assert_eq!(reviewer_tools.len(), reviewers);
        assert!(reviewer_tools.iter().all(|tool| *tool == ToolName::Codex));
    }

    // --- build_review_instruction tests ---

    #[test]
    fn test_build_review_instruction_basic() {
        let result = build_review_instruction("uncommitted", "review-only", "auto", None);
        assert!(result.contains("scope=uncommitted"));
        assert!(result.contains("mode=review-only"));
        assert!(result.contains("security_mode=auto"));
        assert!(result.contains("csa-review skill"));
        // Must NOT contain review instructions or diff content
        assert!(!result.contains("git diff"));
        assert!(!result.contains("Pass 1:"));
    }

    #[test]
    fn test_build_review_instruction_with_context() {
        let result = build_review_instruction(
            "range:main...HEAD",
            "review-only",
            "on",
            Some("/path/to/todo"),
        );
        assert!(result.contains("scope=range:main...HEAD"));
        assert!(result.contains("context=/path/to/todo"));
    }

    #[test]
    fn test_build_review_instruction_fix_mode() {
        let result = build_review_instruction("uncommitted", "review-and-fix", "auto", None);
        assert!(result.contains("mode=review-and-fix"));
    }

    #[test]
    fn test_build_review_instruction_no_diff_content() {
        // Critical: the instruction must not contain actual diff output or review protocol
        let result = build_review_instruction("uncommitted", "review-only", "auto", None);
        assert!(
            result.len() < 200,
            "Instruction should be concise, got {} chars",
            result.len()
        );
    }
}
