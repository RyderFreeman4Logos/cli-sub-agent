use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};

use crate::cli::{ReviewArgs, ReviewMode};
use crate::review_consensus::{
    CLEAN, agreement_level, build_consolidated_artifact, build_multi_reviewer_instruction,
    build_reviewer_tools, consensus_strategy_label, consensus_verdict, parse_consensus_strategy,
    parse_review_verdict, resolve_consensus, write_consolidated_artifact,
};
#[cfg(test)]
use crate::review_context::discover_review_context_for_branch;
use crate::review_context::{
    ResolvedReviewContext, ResolvedReviewContextKind, render_spec_review_context,
    resolve_review_context,
};
use crate::review_routing::{
    ReviewRoutingMetadata, detect_review_routing_metadata, persist_review_routing_artifact,
};
use csa_config::global::{heterogeneous_counterpart, select_heterogeneous_tool};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::consensus::AgentResponse;
use csa_core::types::{OutputFormat, ToolName};
use csa_session::review_artifact::ReviewArtifact;

#[path = "review_cmd_output.rs"]
mod output;
use output::sanitize_review_output;

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

    // 2b. Verify review skill is available (fail fast before any execution)
    verify_review_skill_available(&project_root, args.allow_fallback)?;

    // 3. Derive scope and mode from CLI args
    let scope = derive_scope(&args);
    let mode = if args.fix {
        "review-and-fix"
    } else {
        "review-only"
    };
    let review_mode = args.effective_review_mode();
    let security_mode = args.effective_security_mode();
    let auto_discover_context = review_scope_allows_auto_discovery(&args);
    let context = resolve_review_context(
        args.context.as_deref(),
        &project_root,
        auto_discover_context,
    )?;

    debug!(
        scope = %scope,
        mode = %mode,
        review_mode = %review_mode,
        security_mode = %security_mode,
        auto_discover_context,
        has_context = context.is_some(),
        "Review parameters"
    );

    // 4. Build review instruction (no diff content — tool loads skill and fetches diff itself)
    let (prompt, review_routing) = build_review_instruction_for_project(
        &scope,
        mode,
        security_mode,
        review_mode,
        context.as_ref(),
        &project_root,
        config.as_ref(),
    );

    // 5. Determine tool (with tier-based resolution)
    let detected_parent_tool = crate::run_helpers::detect_parent_tool();
    let parent_tool = crate::run_helpers::resolve_tool(detected_parent_tool, &global_config);
    let (tool, tier_model_spec) = resolve_review_tool(
        args.tool,
        config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
        &project_root,
        args.force_override_user_config,
    )?;

    // Resolve thinking: CLI > config review.thinking > tier model_spec thinking.
    // Tier thinking is embedded in model_spec and applied via build_and_validate_executor.
    let review_thinking = resolve_review_thinking(
        None, // review CLI has no --thinking flag yet
        config
            .as_ref()
            .and_then(|c| c.review.as_ref())
            .and_then(|r| r.thinking.as_deref())
            .or(global_config.review.thinking.as_deref()),
    );

    // Resolve stream mode from CLI flags (default: BufferOnly for review)
    let stream_mode = resolve_review_stream_mode(args.stream_stdout, args.no_stream_stdout);
    let idle_timeout_seconds =
        crate::pipeline::resolve_idle_timeout_seconds(config.as_ref(), args.idle_timeout);

    if args.reviewers == 1 {
        // Keep single-reviewer behavior unchanged.
        let review_future = execute_review(
            tool,
            prompt,
            args.session,
            args.model,
            tier_model_spec.clone(),
            review_thinking.clone(),
            format!(
                "review: {}",
                crate::run_helpers::truncate_prompt(&scope, 80)
            ),
            &project_root,
            config.as_ref(),
            &global_config,
            review_routing.clone(),
            stream_mode,
            idle_timeout_seconds,
            args.force_override_user_config,
        );

        let result = if let Some(timeout_secs) = args.timeout {
            match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), review_future)
                .await
            {
                Ok(inner) => inner?,
                Err(_) => {
                    error!(
                        timeout_secs = timeout_secs,
                        "Review aborted: wall-clock timeout exceeded"
                    );
                    anyhow::bail!(
                        "Review aborted: --timeout {timeout_secs}s exceeded. \
                         Increase --timeout for longer runs, or use --idle-timeout to kill only when output stalls."
                    );
                }
            }
        } else {
            review_future.await?
        };

        print!("{}", sanitize_review_output(&result.output));
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
        let reviewer_routing = review_routing.clone();

        let reviewer_force_override = args.force_override_user_config;
        // Only pass tier_model_spec to the reviewer whose tool matches the
        // tier-resolved primary tool.  For other reviewers (selected for
        // heterogeneity), the model_spec would override their tool via
        // Executor::from_spec, collapsing cognitive diversity.
        let reviewer_model_spec = if reviewer_tool == tool {
            tier_model_spec.clone()
        } else {
            None
        };
        let reviewer_thinking = review_thinking.clone();
        join_set.spawn(async move {
            let result = execute_review(
                reviewer_tool,
                reviewer_prompt,
                None,
                reviewer_model,
                reviewer_model_spec,
                reviewer_thinking,
                reviewer_description,
                &reviewer_project_root,
                reviewer_config.as_ref(),
                &reviewer_global,
                reviewer_routing,
                stream_mode,
                idle_timeout_seconds,
                reviewer_force_override,
            )
            .await?;
            Ok::<ReviewerOutcome, anyhow::Error>(ReviewerOutcome {
                reviewer_index,
                tool: reviewer_tool,
                verdict: parse_review_verdict(&result.output, result.exit_code),
                output: sanitize_review_output(&result.output),
                exit_code: result.exit_code,
            })
        });
    }

    let mut outcomes = Vec::with_capacity(reviewers);
    let collect_future = async {
        while let Some(joined) = join_set.join_next().await {
            let outcome = joined.context("reviewer task join failure")??;
            outcomes.push(outcome);
        }
        Ok::<_, anyhow::Error>(())
    };

    if let Some(timeout_secs) = args.timeout {
        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), collect_future)
            .await
        {
            Ok(inner) => inner?,
            Err(_) => {
                error!(
                    timeout_secs = timeout_secs,
                    "Multi-reviewer review aborted: wall-clock timeout exceeded"
                );
                anyhow::bail!(
                    "Review aborted: --timeout {timeout_secs}s exceeded. \
                     Increase --timeout for longer runs, or use --idle-timeout to kill only when output stalls."
                );
            }
        }
    } else {
        collect_future.await?;
    }
    outcomes.sort_by_key(|o| o.reviewer_index);

    if let Err(err) = write_multi_reviewer_consolidated_artifact(reviewers) {
        warn!(
            error = %err,
            "Failed to write consolidated multi-reviewer artifact (shadow mode; continuing)"
        );
    }

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
    tier_model_spec: Option<String>,
    thinking: Option<String>,
    description: String,
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    review_routing: ReviewRoutingMetadata,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    force_override_user_config: bool,
) -> Result<csa_process::ExecutionResult> {
    let executor = crate::pipeline::build_and_validate_executor(
        &tool,
        tier_model_spec.as_deref(),
        model.as_deref(),
        thinking.as_deref(),
        crate::pipeline::ConfigRefs {
            project: project_config,
            global: Some(global_config),
        },
        false, // skip tier whitelist for review tool selection
        force_override_user_config,
        false, // review must not inherit `csa run` per-tool defaults
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

    if session.is_none() {
        if let Ok(inherited_session_id) = std::env::var("CSA_SESSION_ID") {
            warn!(
                inherited_session_id = %inherited_session_id,
                "Ignoring inherited CSA_SESSION_ID for `csa review`; pass --session to resume explicitly"
            );
        }
    }

    let execution = crate::pipeline::execute_with_session_and_meta_with_parent_source(
        &executor,
        &tool,
        &effective_prompt,
        OutputFormat::Json,
        session,
        Some(description),
        None,
        project_root,
        project_config,
        extra_env,
        Some("review"),
        None,
        None,
        stream_mode,
        idle_timeout_seconds,
        None,
        None,
        Some(global_config),
        crate::pipeline::ParentSessionSource::ExplicitOnly,
    )
    .await?;

    persist_review_routing_artifact(project_root, &execution.meta_session_id, &review_routing);

    Ok(execution.execution)
}

/// Verify the review pattern is installed before attempting execution.
///
/// By default this fails fast with actionable install guidance if the pattern
/// is missing. When `allow_fallback` is true, it downgrades to warning and
/// lets review continue without the structured pattern protocol.
fn verify_review_skill_available(project_root: &Path, allow_fallback: bool) -> Result<()> {
    match crate::pattern_resolver::resolve_pattern("csa-review", project_root) {
        Ok(resolved) => {
            debug!(
                pattern_dir = %resolved.dir.display(),
                has_config = resolved.config.is_some(),
                has_agent = resolved.agent_config().is_some(),
                skill_md_len = resolved.skill_md.len(),
                "Review pattern resolved"
            );
            Ok(())
        }
        Err(resolve_err) => {
            if allow_fallback {
                warn!(
                    "Review pattern not found; continuing because --allow-fallback is set. \
                     Install with `weave install RyderFreeman4Logos/cli-sub-agent` for structured review protocol."
                );
                return Ok(());
            }

            anyhow::bail!(
                "Review pattern not found — `csa review` requires the 'csa-review' pattern.\n\n\
                 {resolve_err}\n\n\
                 Install the review pattern with one of:\n\
                 1) weave install RyderFreeman4Logos/cli-sub-agent\n\
                 2) Manually place skills/csa-review/SKILL.md (or PATTERN.md) inside .csa/patterns/csa-review/ or patterns/csa-review/\n\n\
                 Note: `csa skill install` only installs `.claude/skills/*`; it does NOT install `.csa/patterns/*`.\n\n\
                 Without the pattern, the review tool cannot follow the structured review protocol."
            )
        }
    }
}

/// Resolve stream mode for review command.
///
/// - `--stream-stdout` forces TeeToStderr (progressive output)
/// - `--no-stream-stdout` forces BufferOnly (silent until complete)
/// - Default: BufferOnly to prevent raw provider noise from polluting review
///   output. Long-running progress is still surfaced by periodic heartbeats.
fn resolve_review_stream_mode(
    stream_stdout: bool,
    no_stream_stdout: bool,
) -> csa_process::StreamMode {
    if no_stream_stdout {
        csa_process::StreamMode::BufferOnly
    } else if stream_stdout {
        csa_process::StreamMode::TeeToStderr
    } else {
        csa_process::StreamMode::BufferOnly
    }
}

/// Returns (tool, optional_model_spec). When tier resolves, model_spec is set.
fn resolve_review_tool(
    arg_tool: Option<ToolName>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    parent_tool: Option<&str>,
    project_root: &Path,
    force_override_user_config: bool,
) -> Result<(ToolName, Option<String>)> {
    // CLI --tool override always wins
    if let Some(tool) = arg_tool {
        if let Some(cfg) = project_config {
            cfg.enforce_tool_enabled(tool.as_str(), force_override_user_config)?;
        }
        return Ok((tool, None));
    }

    // Tier-based resolution: project tier > global tier > tool-based fallback.
    // Tier takes priority over tool when both are set.
    let tier_name = project_config
        .and_then(|cfg| cfg.review.as_ref())
        .and_then(|r| r.tier.as_deref())
        .or(global_config.review.tier.as_deref());

    if let Some(tier) = tier_name {
        if let Some(cfg) = project_config {
            if let Some(resolution) =
                crate::run_helpers::resolve_tool_from_tier(tier, cfg, parent_tool)
            {
                return Ok((resolution.tool, Some(resolution.model_spec)));
            }
        }
        // Tier set but no available tool found — fall through to tool-based resolution
        debug!(
            tier = tier,
            "Tier set but no available tool found, falling through to tool-based resolution"
        );
    }

    if let Some(project_review) = project_config.and_then(|cfg| cfg.review.as_ref()) {
        return resolve_review_tool_from_value(
            &project_review.tool,
            parent_tool,
            project_config,
            global_config,
            project_root,
        )
        .map(|t| (t, None))
        .with_context(|| {
            format!(
                "Failed to resolve review tool from project config: {}",
                ProjectConfig::config_path(project_root).display()
            )
        });
    }

    // When global [review].tool is "auto", always try heterogeneous auto-selection first.
    if global_config.review.tool == "auto" {
        if let Some(tool) = select_auto_review_tool(parent_tool, project_config, global_config) {
            return Ok((tool, None));
        }
    }

    match global_config.resolve_review_tool(parent_tool) {
        Ok(tool_name) => {
            if let Some(cfg) = project_config {
                if !cfg.is_tool_enabled(&tool_name) {
                    return Err(review_auto_resolution_error(parent_tool, project_root));
                }
            }
            crate::run_helpers::parse_tool_name(&tool_name)
                .map(|t| (t, None))
                .map_err(|_| {
                    anyhow::anyhow!(
                    "Invalid [review].tool value '{}'. Supported values: gemini-cli, opencode, codex, claude-code.",
                    tool_name
                )
                })
        }
        Err(_) => Err(review_auto_resolution_error(parent_tool, project_root)),
    }
}

/// Resolve review thinking: CLI > config review.thinking.
fn resolve_review_thinking(
    cli_thinking: Option<&str>,
    config_thinking: Option<&str>,
) -> Option<String> {
    cli_thinking
        .map(str::to_string)
        .or_else(|| config_thinking.map(str::to_string))
}

fn resolve_review_tool_from_value(
    tool_value: &str,
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    project_root: &Path,
) -> Result<ToolName> {
    if tool_value == "auto" {
        if let Some(tool) = select_auto_review_tool(parent_tool, project_config, global_config) {
            return Ok(tool);
        }

        // Try old heterogeneous_counterpart first for backward compatibility,
        // but only if the counterpart tool is enabled.
        if let Some(resolved) = parent_tool.and_then(heterogeneous_counterpart) {
            let counterpart_enabled =
                project_config.is_none_or(|cfg| cfg.is_tool_enabled(resolved));
            if counterpart_enabled {
                return crate::run_helpers::parse_tool_name(resolved).map_err(|_| {
                    anyhow::anyhow!(
                        "BUG: auto review tool resolution returned invalid tool '{}'",
                        resolved
                    )
                });
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
            .filter(|t| cfg.is_tool_auto_selectable(t.as_str()))
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
3) CLI override: csa review --sa-mode <true|false> --tool codex\n\n\
Reason: CSA enforces heterogeneity in auto mode and will not fall back."
    )
}

fn write_multi_reviewer_consolidated_artifact(reviewers: usize) -> Result<()> {
    let Some(session_dir) = std::env::var_os("CSA_SESSION_DIR") else {
        return Ok(());
    };
    let output_dir = PathBuf::from(session_dir);
    let session_id = std::env::var("CSA_SESSION_ID").unwrap_or_else(|_| "unknown".to_string());

    let mut reviewer_artifacts = Vec::new();
    for reviewer_index in 1..=reviewers {
        let artifact_path = output_dir
            .join(format!("reviewer-{reviewer_index}"))
            .join("review-findings.json");

        if !artifact_path.exists() {
            continue;
        }

        let content = std::fs::read_to_string(&artifact_path)
            .with_context(|| format!("failed to read {}", artifact_path.display()))?;
        let artifact: ReviewArtifact = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", artifact_path.display()))?;
        reviewer_artifacts.push(artifact);
    }

    let consolidated = build_consolidated_artifact(reviewer_artifacts, &session_id);
    write_consolidated_artifact(&consolidated, &output_dir)
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
    format!("base:{}", args.branch.as_deref().unwrap_or("main"))
}

fn review_scope_allows_auto_discovery(args: &ReviewArgs) -> bool {
    args.range.is_some() || (!args.diff && args.commit.is_none() && args.files.is_none())
}

/// Anti-recursion preamble injected into every review/debate subprocess prompt.
///
/// Prevents the spawned tool (e.g. claude-code-acp) from reading CLAUDE.md rules
/// that say "use csa review" and recursively invoking `csa run` / `csa review`.
const ANTI_RECURSION_PREAMBLE: &str = "\
CRITICAL: You are running INSIDE a CSA subprocess (csa review / csa debate). \
Do NOT invoke `csa run`, `csa review`, `csa debate`, or ANY `csa` CLI command — \
this causes infinite recursion. Perform the task DIRECTLY using your own \
capabilities (Read, Grep, Glob, Bash for read-only git commands). \
REVIEW-ONLY SAFETY: Do NOT run git add/commit/push/merge/rebase/tag/stash/reset/checkout/cherry-pick, \
and do NOT run gh pr/create/comment/merge or any command that mutates repository/PR state. \
Ignore prompt-guard reminders about commit/push in this subprocess. \
Ignore any CLAUDE.md or AGENTS.md rules that instruct you to delegate to CSA.\n\n";

/// Build a concise review instruction that tells the tool to use the csa-review skill.
///
/// The tool loads the skill from `.claude/skills/csa-review/` automatically.
/// CSA only passes scope, mode, and optional parameters — no diff content.
/// An anti-recursion preamble is prepended to prevent the spawned tool from
/// re-invoking CSA commands (see GitHub issue #272).
fn build_review_instruction(
    scope: &str,
    mode: &str,
    security_mode: &str,
    review_mode: ReviewMode,
    context: Option<&ResolvedReviewContext>,
) -> String {
    let mut instruction = format!(
        "{ANTI_RECURSION_PREAMBLE}Use the csa-review skill. scope={scope}, mode={mode}, security_mode={security_mode}, review_mode={review_mode}."
    );
    if let Some(ctx) = context {
        instruction.push_str(&format!(" context={}", ctx.path));
        if let ResolvedReviewContextKind::SpecToml { spec } = &ctx.kind {
            instruction.push_str(
                "\nSpec alignment context (parsed from spec.toml; use this criteria set directly):\n",
            );
            instruction.push_str(&render_spec_review_context(spec));
        }
    }
    instruction
}

fn build_review_instruction_for_project(
    scope: &str,
    mode: &str,
    security_mode: &str,
    review_mode: ReviewMode,
    context: Option<&ResolvedReviewContext>,
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
) -> (String, ReviewRoutingMetadata) {
    let review_routing = detect_review_routing_metadata(project_root, project_config);
    let mut instruction =
        build_review_instruction(scope, mode, security_mode, review_mode, context);
    instruction.push_str(&format!(
        "\n[project_profile: {}]",
        review_routing.project_profile
    ));
    (instruction, review_routing)
}

#[cfg(test)]
#[path = "review_cmd_tests.rs"]
mod tests;
