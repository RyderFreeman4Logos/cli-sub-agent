use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{debug, warn};

use crate::cli::{ReviewArgs, ReviewMode};
use crate::review_consensus::{build_consolidated_artifact, write_consolidated_artifact};
use crate::review_context::{
    ResolvedReviewContext, ResolvedReviewContextKind, render_spec_review_context,
};
use crate::review_routing::{ReviewRoutingMetadata, detect_review_routing_metadata};
use csa_config::global::{heterogeneous_counterpart, select_heterogeneous_tool};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_session::review_artifact::ReviewArtifact;

/// Verify the review pattern is installed before attempting execution.
///
/// By default this fails fast with actionable install guidance if the pattern
/// is missing. When `allow_fallback` is true, it downgrades to warning and
/// lets review continue without the structured pattern protocol.
pub(crate) fn verify_review_skill_available(
    project_root: &Path,
    allow_fallback: bool,
) -> Result<()> {
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
pub(crate) fn resolve_review_stream_mode(
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
pub(crate) fn resolve_review_tool(
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
pub(crate) fn resolve_review_thinking(
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

pub(crate) fn write_multi_reviewer_consolidated_artifact(reviewers: usize) -> Result<()> {
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
pub(crate) fn derive_scope(args: &ReviewArgs) -> String {
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

pub(crate) fn review_scope_allows_auto_discovery(args: &ReviewArgs) -> bool {
    args.range.is_some() || (!args.diff && args.commit.is_none() && args.files.is_none())
}

/// Anti-recursion preamble injected into every review/debate subprocess prompt.
///
/// Prevents the spawned tool (e.g. claude-code-acp) from reading CLAUDE.md rules
/// that say "use csa review" and recursively invoking `csa run` / `csa review`.
pub(crate) const ANTI_RECURSION_PREAMBLE: &str = "\
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
pub(crate) fn build_review_instruction(
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

pub(crate) fn build_review_instruction_for_project(
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
