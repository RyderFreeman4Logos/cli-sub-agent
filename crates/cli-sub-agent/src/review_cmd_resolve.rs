use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{debug, warn};

use crate::cli::{ReviewArgs, ReviewMode};
use crate::review_consensus::{build_consolidated_artifact, write_consolidated_artifact};
use crate::review_context::{
    ResolvedReviewContext, ResolvedReviewContextKind, discover_prior_round_assumptions,
    discover_review_checklist, render_spec_review_context,
};
use crate::review_prior_rounds::REVIEW_FINDINGS_TOML_INSTRUCTION;
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_review_tool(
    arg_tool: Option<ToolName>,
    arg_model_spec: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    parent_tool: Option<&str>,
    project_root: &Path,
    force_override_user_config: bool,
    cli_tier: Option<&str>,
    force_ignore_tier_setting: bool,
) -> Result<(ToolName, Option<String>)> {
    let tiers_configured = project_config.is_some_and(|c| !c.tiers.is_empty());
    let bypass_tier = force_ignore_tier_setting || force_override_user_config;

    crate::run_helpers::validate_tool_tier_override_flags(
        arg_tool.is_some(),
        cli_tier,
        force_ignore_tier_setting,
    )?;
    crate::run_helpers::validate_model_spec_tier_conflict(arg_model_spec, cli_tier, "review")?;

    if let Some(model_spec) = arg_model_spec {
        let (tool, resolved_model_spec, _) = crate::run_helpers::resolve_tool_and_model(
            arg_tool,
            Some(model_spec),
            None,
            project_config,
            project_root,
            false,
            force_override_user_config,
            false,
            cli_tier,
            force_ignore_tier_setting,
            false,
        )?;
        return Ok((tool, resolved_model_spec));
    }

    // Enforce tier routing: block direct --tool when tiers are configured,
    // unless --force-ignore-tier-setting (or --force-override-user-config) is active.
    if tiers_configured && !bypass_tier && cli_tier.is_none() && arg_tool.is_some() {
        let cfg = project_config.unwrap();
        let available: Vec<&str> = cfg.tiers.keys().map(|k| k.as_str()).collect();
        let alias_hint = cfg.format_tier_aliases();
        anyhow::bail!(
            "Direct --tool is restricted when tiers are configured. \
             Use --tier <name> to specify which tier's model/thinking config to use, \
             or add --force-ignore-tier-setting to override. \
             Available tiers: [{}]{alias_hint}",
            available.join(", ")
        );
    }

    let tier_name = resolve_review_tier_name(
        project_config,
        global_config,
        cli_tier,
        force_override_user_config,
        force_ignore_tier_setting,
    )?;

    if let Some(tool) = arg_tool {
        if let Some(ref tier) = tier_name
            && let Some(cfg) = project_config
        {
            let resolution = crate::run_helpers::resolve_requested_tool_from_tier(
                tier,
                cfg,
                parent_tool,
                tool,
                force_override_user_config,
                &[],
            )?;
            return Ok((resolution.tool, Some(resolution.model_spec)));
        }

        if let Some(cfg) = project_config {
            cfg.enforce_tool_enabled(tool.as_str(), force_override_user_config)?;
        }
        return Ok((tool, None));
    }

    // Compute effective whitelist from tool selection (project > global).
    // IMPORTANT (#648): When [review].tool is set, it acts as a whitelist filter
    // on the tier's model list, silently narrowing a multi-tool tier to only the
    // specified tool(s). To use the full tier fallback chain, set tool = "auto".
    let effective_whitelist = project_config
        .and_then(|cfg| cfg.review.as_ref())
        .map(|r| &r.tool)
        .unwrap_or(&global_config.review.tool);
    let whitelist = effective_whitelist.whitelist();

    if let Some(ref tier) = tier_name {
        let cfg = project_config.ok_or_else(|| {
            anyhow::anyhow!(
                "Review tier '{}' is configured, but no tier definitions are available. \
                 Run `csa init --full` or define [tiers.*] in config.",
                tier
            )
        })?;

        let tier_tools = cfg.list_tools_in_tier(tier);
        if let Some(wl) = whitelist {
            let matching_tools: Vec<&str> = tier_tools
                .iter()
                .filter(|(tool_name, _)| wl.iter().any(|allowed| allowed == tool_name))
                .map(|(tool_name, _)| tool_name.as_str())
                .collect();
            if matching_tools.is_empty() {
                let tier_tool_names: Vec<&str> = tier_tools
                    .iter()
                    .map(|(tool_name, _)| tool_name.as_str())
                    .collect();
                anyhow::bail!(
                    "Tier '{}' has no tools matching [review].tool whitelist [{}]. \
                     The active review tier remains authoritative.\n\
                     Tier tools: [{}].\n\
                     Update [review].tool or choose a different tier.",
                    tier,
                    wl.join(", "),
                    tier_tool_names.join(", ")
                );
            }
        }

        if let Some(resolution) =
            crate::run_helpers::resolve_tool_from_tier(tier, cfg, parent_tool, whitelist, &[])
        {
            return Ok((resolution.tool, Some(resolution.model_spec)));
        }

        let filtered_tools =
            crate::run_helpers::collect_available_tier_models(tier, cfg, whitelist, &[]);
        let configured_tools: Vec<&str> = tier_tools
            .iter()
            .map(|(tool_name, _)| tool_name.as_str())
            .collect();
        let available_tools: Vec<&str> = filtered_tools
            .iter()
            .map(|resolution| resolution.tool.as_str())
            .collect();
        anyhow::bail!(
            "Tier '{}' resolved for review, but none of its tools are currently available.\n\
             Configured tier tools: [{}].\n\
             Available tier tools after enablement/install checks: [{}].",
            tier,
            configured_tools.join(", "),
            available_tools.join(", ")
        );
    }

    if let Some(project_review) = project_config.and_then(|cfg| cfg.review.as_ref()) {
        return resolve_review_tool_from_selection(
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

    // Global config tool selection
    resolve_review_tool_from_selection(
        &global_config.review.tool,
        parent_tool,
        project_config,
        global_config,
        project_root,
    )
    .map(|t| (t, None))
}

pub(crate) fn resolve_review_tier_name(
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    cli_tier: Option<&str>,
    force_override_user_config: bool,
    force_ignore_tier_setting: bool,
) -> Result<Option<String>> {
    let bypass_tier = force_ignore_tier_setting || force_override_user_config;

    if let Some(cli) = cli_tier {
        if let Some(cfg) = project_config {
            if let Some(canonical) = cfg.resolve_tier_selector(cli) {
                return Ok(Some(canonical));
            }
            if bypass_tier {
                return Ok(Some(cli.to_string()));
            }
            let available: Vec<&str> = cfg.tiers.keys().map(|k| k.as_str()).collect();
            let alias_hint = cfg.format_tier_aliases();
            let suggest_hint = cfg
                .suggest_tier(cli)
                .map(|s| format!("\nDid you mean '{s}'?"))
                .unwrap_or_default();
            anyhow::bail!(
                "Tier selector '{}' not found.\n\
                 Available tiers: [{}]{alias_hint}{suggest_hint}",
                cli,
                available.join(", ")
            );
        }
        return Ok(Some(cli.to_string()));
    }

    Ok(project_config
        .and_then(|cfg| cfg.review.as_ref())
        .and_then(|r| r.tier.as_deref())
        .or(global_config.review.tier.as_deref())
        .map(|s| s.to_string()))
}

pub(crate) fn resolve_review_model(
    cli_model: Option<&str>,
    config_model: Option<&str>,
    model_spec_active: bool,
) -> Option<String> {
    cli_model.map(str::to_string).or_else(|| {
        (!model_spec_active)
            .then_some(config_model)
            .flatten()
            .map(str::to_string)
    })
}

/// Resolve review thinking: CLI > config review.thinking when no tier is active.
pub(crate) fn resolve_review_thinking(
    cli_thinking: Option<&str>,
    config_thinking: Option<&str>,
    model_spec_active: bool,
) -> Option<String> {
    cli_thinking.map(str::to_string).or_else(|| {
        (!model_spec_active)
            .then_some(config_thinking)
            .flatten()
            .map(str::to_string)
    })
}

fn resolve_review_tool_from_selection(
    selection: &csa_config::ToolSelection,
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    project_root: &Path,
) -> Result<ToolName> {
    // Single direct tool (not "auto")
    if let Some(tool_name) = selection.as_single() {
        let tool = crate::run_helpers::parse_tool_name(tool_name).map_err(|_| {
            anyhow::anyhow!(
                "Invalid [review].tool value '{tool_name}'. Supported values: auto, gemini-cli, opencode, codex, claude-code."
            )
        })?;
        // Verify the tool is enabled in the project config
        if let Some(cfg) = project_config
            && !cfg.is_tool_enabled(tool_name)
        {
            anyhow::bail!(
                "[review].tool = '{tool_name}' is disabled in project config. \
                     Enable it in [tools.{tool_name}] or change [review].tool."
            );
        }
        return Ok(tool);
    }

    // Auto or whitelist — try heterogeneous auto-selection with optional filter
    let whitelist = selection.whitelist();
    if let Some(tool) =
        select_auto_review_tool(parent_tool, project_config, global_config, whitelist)
    {
        return Ok(tool);
    }

    // Legacy counterpart fallback (only for true auto, not whitelist)
    if whitelist.is_none()
        && let Some(resolved) = parent_tool.and_then(heterogeneous_counterpart)
    {
        let counterpart_enabled = project_config.is_none_or(|cfg| cfg.is_tool_enabled(resolved));
        let counterpart_available =
            crate::run_helpers::is_tool_binary_available_for_config(resolved, project_config);
        if counterpart_enabled && counterpart_available {
            return crate::run_helpers::parse_tool_name(resolved).map_err(|_| {
                anyhow::anyhow!(
                    "BUG: auto review tool resolution returned invalid tool '{resolved}'"
                )
            });
        }
    }

    Err(review_auto_resolution_error(parent_tool, project_root))
}

fn select_auto_review_tool(
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    whitelist: Option<&[String]>,
) -> Option<ToolName> {
    let parent_str = parent_tool?;
    let parent_tool_name = crate::run_helpers::parse_tool_name(parent_str).ok()?;
    let enabled_tools: Vec<_> = if let Some(cfg) = project_config {
        let tools: Vec<_> = csa_config::global::all_known_tools()
            .iter()
            .filter(|t| cfg.is_tool_auto_selectable(t.as_str()))
            .filter(|t| {
                crate::run_helpers::is_tool_binary_available_for_config(t.as_str(), project_config)
            })
            .filter(|t| whitelist.is_none_or(|wl| wl.iter().any(|w| w == t.as_str())))
            .copied()
            .collect();
        csa_config::global::sort_tools_by_effective_priority(&tools, project_config, global_config)
    } else {
        let all = csa_config::global::all_known_tools();
        let tools: Vec<_> = all
            .iter()
            .filter(|t| {
                crate::run_helpers::is_tool_binary_available_for_config(t.as_str(), project_config)
            })
            .filter(|t| whitelist.is_none_or(|wl| wl.iter().any(|w| w == t.as_str())))
            .copied()
            .collect();
        csa_config::global::sort_tools_by_effective_priority(&tools, project_config, global_config)
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

/// Review-only safety preamble injected into every review subprocess prompt.
///
/// Constrains the reviewer tool (e.g. claude-code-acp, codex-acp) to read-only
/// operations on the repository: no mutations via git/gh. The reviewer may
/// perform the task directly or, when it clearly halves the work, delegate
/// sub-tasks via `csa` — the depth-aware guard in `pipeline::prompt_guard`
/// and the hard ceiling in `load_and_validate` enforce the recursion contract
/// (see `MAX_RECURSION_DEPTH`), so prompt-level blanket prohibition here is
/// both redundant and counter-productive for the documented fractal-recursion
/// pattern.
pub(crate) const ANTI_RECURSION_PREAMBLE: &str = "\
CONTEXT: You are running INSIDE a CSA subprocess (csa review / csa debate). \
Perform the review task DIRECTLY using your own capabilities \
(Read, Grep, Glob, Bash for read-only git commands). \
REVIEW-ONLY SAFETY: Do NOT run git add/commit/push/merge/rebase/tag/stash/reset/checkout/cherry-pick, \
and do NOT run gh pr/create/comment/merge or any command that mutates repository/PR state. \
Ignore prompt-guard reminders about commit/push in this subprocess.\n\n";

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
        "{ANTI_RECURSION_PREAMBLE}Use the csa-review skill. scope={scope}, mode={mode}, security_mode={security_mode}, review_mode={review_mode}. Emit exactly one final verdict token: PASS, FAIL, SKIP, or UNCERTAIN."
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
    append_design_anchor(&mut instruction);
    instruction
}

pub(crate) fn build_review_instruction_for_project(
    scope: &str,
    mode: &str,
    security_mode: &str,
    review_mode: ReviewMode,
    context: Option<&ResolvedReviewContext>,
    project_root: &Path,
    options: ReviewProjectPromptOptions<'_>,
) -> (String, ReviewRoutingMetadata) {
    let review_routing = detect_review_routing_metadata(project_root, options.project_config);
    let mut instruction =
        build_review_instruction(scope, mode, security_mode, review_mode, context);
    instruction.push_str(&format!(
        "\n[project_profile: {}]",
        review_routing.project_profile
    ));

    // Inject project-specific review checklist if present
    if let Some(checklist) = discover_review_checklist(project_root) {
        instruction.push_str("\n\n<review-checklist>\n");
        instruction.push_str(&checklist);
        instruction.push_str("\n</review-checklist>");
    }

    // Inject prior-round assumptions when a previous review session exists on
    // the same branch (#764). Skipped on first-round reviews.
    let branch = current_git_branch_for_review(project_root);
    if let Some(branch) = branch.as_deref()
        && let Some(iteration_context) =
            crate::review_consensus::review_iteration::render_review_iteration_context(
                project_root,
                branch,
            )
    {
        instruction.push_str("\n\n");
        instruction.push_str(&iteration_context);
    }
    if let Some(prior) = discover_prior_round_assumptions(project_root, branch.as_deref(), None) {
        instruction.push_str(&prior);
    }
    if let Some(prior_rounds_section) = options.prior_rounds_section {
        instruction.push_str("\n\n");
        instruction.push_str(prior_rounds_section);
    }
    instruction.push_str("\n\n");
    instruction.push_str(REVIEW_FINDINGS_TOML_INSTRUCTION);

    (instruction, review_routing)
}

fn current_git_branch_for_review(project_root: &Path) -> Option<String> {
    let backend = csa_session::vcs_backends::create_vcs_backend(project_root);
    backend.current_branch(project_root).ok().flatten()
}

fn append_design_anchor(prompt: &mut String) {
    if prompt.contains("## Design preferences vs correctness bugs") {
        return;
    }
    prompt.push_str("\n\n");
    prompt.push_str(crate::review_consensus::review_design_anchor::REVIEW_DESIGN_PREFERENCE_ANCHOR);
}

pub(crate) struct ReviewProjectPromptOptions<'a> {
    pub(crate) project_config: Option<&'a ProjectConfig>,
    pub(crate) prior_rounds_section: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::write_multi_reviewer_consolidated_artifact;
    use csa_core::env::CSA_SESSION_DIR_ENV_KEY;
    use csa_session::review_artifact::{Finding, ReviewArtifact, Severity, SeveritySummary};
    use std::fs;
    use std::sync::Mutex;
    use tempfile::tempdir;

    static REVIEW_RESOLVE_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct ScopedEnvVar {
        key: &'static str,
        original: Option<String>,
    }

    impl ScopedEnvVar {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: test-scoped env mutation guarded by REVIEW_RESOLVE_ENV_LOCK.
            unsafe { std::env::set_var(key, value) };
            Self { key, original }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            // SAFETY: test-scoped env mutation guarded by REVIEW_RESOLVE_ENV_LOCK.
            unsafe {
                match self.original.take() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn write_multi_reviewer_consolidated_artifact_reads_parent_reviewer_dir() {
        let _env_lock = REVIEW_RESOLVE_ENV_LOCK
            .lock()
            .expect("review resolve env lock poisoned");
        let temp = tempdir().expect("tempdir should be created");
        let session_dir = temp.path().display().to_string();
        let _session_dir_guard = ScopedEnvVar::set(CSA_SESSION_DIR_ENV_KEY, &session_dir);
        let _session_id_guard = ScopedEnvVar::set("CSA_SESSION_ID", "01PARENTSESSION000000000000");

        let reviewer_dir = temp.path().join("reviewer-1");
        fs::create_dir_all(&reviewer_dir).expect("reviewer dir should be created");
        let findings = vec![Finding {
            severity: Severity::High,
            fid: "FID-1".to_string(),
            file: "src/lib.rs".to_string(),
            line: Some(7),
            rule_id: "rule.review.parent-path".to_string(),
            summary: "parent-path finding".to_string(),
            engine: "reviewer".to_string(),
        }];
        let artifact = ReviewArtifact {
            severity_summary: SeveritySummary::from_findings(&findings),
            findings: findings.clone(),
            review_mode: Some("diff".to_string()),
            schema_version: "1.0".to_string(),
            session_id: "01CHILDSESSION0000000000000".to_string(),
            timestamp: chrono::Utc::now(),
        };
        fs::write(
            reviewer_dir.join("review-findings.json"),
            serde_json::to_vec_pretty(&artifact).expect("artifact should serialize"),
        )
        .expect("review artifact should be written");

        write_multi_reviewer_consolidated_artifact(1)
            .expect("consolidated artifact should be produced");

        let consolidated_path = temp.path().join("review-findings-consolidated.json");
        let consolidated: ReviewArtifact = serde_json::from_str(
            &fs::read_to_string(&consolidated_path).expect("consolidated artifact should exist"),
        )
        .expect("consolidated artifact should parse");
        assert_eq!(consolidated.findings.len(), 1);
        assert_eq!(consolidated.findings[0].fid, "FID-1");
        assert_eq!(consolidated.severity_summary.high, 1);
    }
}
