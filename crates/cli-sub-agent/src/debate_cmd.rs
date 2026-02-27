use std::io::IsTerminal;

use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, error, warn};

use crate::cli::DebateArgs;
use crate::debate_errors::{DebateErrorKind, classify_execution_error, classify_execution_outcome};
use crate::run_helpers::read_prompt;
use csa_config::global::{heterogeneous_counterpart, select_heterogeneous_tool};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;

use crate::debate_cmd_output::{
    append_debate_artifacts_to_result, extract_debate_summary, format_debate_stdout_summary,
    persist_debate_output_artifacts, render_debate_output,
};

/// Debate execution mode indicating model diversity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DebateMode {
    /// Different model families (e.g., Claude vs OpenAI) — full cognitive diversity.
    Heterogeneous,
    /// Same tool used for both Proposer and Critic — degraded diversity.
    SameModelAdversarial,
}

pub(crate) async fn handle_debate(args: DebateArgs, current_depth: u32) -> Result<i32> {
    // 1. Determine project root
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;

    // 2. Load config and validate recursion depth
    let Some((config, global_config)) =
        crate::pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };

    // 2b. Verify debate skill is available (fail fast before any execution)
    verify_debate_skill_available(&project_root)?;

    // 3. Read question (from arg or stdin)
    let question = read_prompt(args.question)?;

    // 4. Build debate instruction (parameter passing — tool loads debate skill)
    let prompt = build_debate_instruction(&question, args.session.is_some(), args.rounds);

    // 5. Determine tool (heterogeneous enforcement)
    let detected_parent_tool = crate::run_helpers::detect_parent_tool();
    let parent_tool = crate::run_helpers::resolve_tool(detected_parent_tool, &global_config);
    let (tool, debate_mode) = resolve_debate_tool(
        args.tool,
        config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
        &project_root,
        args.force_override_user_config,
    )?;
    if debate_mode == DebateMode::SameModelAdversarial {
        warn!(
            tool = %tool.as_str(),
            "Falling back to same-model adversarial debate — heterogeneous models unavailable. \
             Cognitive diversity is degraded."
        );
    }
    let thinking = resolve_debate_thinking(
        args.thinking.as_deref(),
        global_config.debate.thinking.as_deref(),
    );

    // 6. Build executor and validate tool
    let executor = crate::pipeline::build_and_validate_executor(
        &tool,
        None,
        args.model.as_deref(),
        thinking.as_deref(),
        crate::pipeline::ConfigRefs {
            project: config.as_ref(),
            global: Some(&global_config),
        },
        false, // skip tier whitelist for debate tool selection
        args.force_override_user_config,
    )
    .await?;

    // 7. Get env injection from global config
    let extra_env = global_config.env_vars(executor.tool_name());
    let idle_timeout_seconds =
        crate::pipeline::resolve_idle_timeout_seconds(config.as_ref(), args.idle_timeout);

    // Resolve stream mode from CLI flags (default: BufferOnly for debate)
    let stream_mode = resolve_debate_stream_mode(args.stream_stdout, args.no_stream_stdout);

    // 8. Acquire global slot to enforce concurrency limit
    let _slot_guard = crate::pipeline::acquire_slot(&executor, &global_config)?;

    // 9. Execute with session (with optional absolute timeout + transient retry)
    let description = format!(
        "debate: {}",
        crate::run_helpers::truncate_prompt(&question, 80)
    );
    let timeout_seconds =
        resolve_debate_timeout_seconds(args.timeout, Some(global_config.debate.timeout_seconds));
    let wall_clock_start = Instant::now();
    let mut retry_count = 0u8;
    let mut first_error_context: Option<String> = None;
    let mut resume_session = args.session.clone();

    let execution = loop {
        ensure_debate_wall_clock_within_timeout(wall_clock_start, timeout_seconds)?;

        let execute_future = crate::pipeline::execute_with_session_and_meta(
            &executor,
            &tool,
            &prompt,
            resume_session.clone(),
            Some(description.clone()),
            None,
            &project_root,
            config.as_ref(),
            extra_env,
            Some("debate"),
            None, // debate does not use tier-based selection
            None, // debate does not override context loading options
            stream_mode,
            idle_timeout_seconds,
            None,
            Some(&global_config),
        );

        let execute_result = if let Some(timeout_secs) = timeout_seconds {
            match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), execute_future)
                .await
            {
                Ok(inner) => inner,
                Err(_) => Err(anyhow::anyhow!(
                    "Debate aborted: --timeout {timeout_secs}s exceeded. \
                     Increase --timeout for longer runs, or rely on --idle-timeout to terminate stalled output."
                )),
            }
        } else {
            execute_future.await
        };

        let executed = match execute_result {
            Ok(execution) => execution,
            Err(err) => {
                let session_dir = resume_session.as_deref().and_then(|session_id| {
                    csa_session::get_session_dir(&project_root, session_id).ok()
                });
                match classify_execution_error(&err, session_dir.as_deref()) {
                    DebateErrorKind::StillWorking => {
                        wait_for_still_working_backoff().await;
                        continue;
                    }
                    DebateErrorKind::Transient(reason)
                        if should_retry_debate_after_error(
                            &DebateErrorKind::Transient(reason.clone()),
                            retry_count,
                        ) =>
                    {
                        if first_error_context.is_none() {
                            first_error_context = Some(err.to_string());
                        }
                        retry_count += 1;
                        warn!("Retrying debate after transient error: {reason}");
                        continue;
                    }
                    _ => {
                        error!("Debate aborted before completion: {err}");
                        return Err(err);
                    }
                }
            }
        };

        resume_session = Some(executed.meta_session_id.clone());
        if executed.execution.exit_code == 0 {
            break executed;
        }

        let session_dir = csa_session::get_session_dir(&project_root, &executed.meta_session_id)?;
        let session_state =
            csa_session::load_session(&project_root, &executed.meta_session_id).ok();
        match classify_execution_outcome(&executed.execution, session_state.as_ref(), &session_dir)
        {
            DebateErrorKind::StillWorking => {
                wait_for_still_working_backoff().await;
                continue;
            }
            DebateErrorKind::Transient(reason)
                if should_retry_debate_after_error(
                    &DebateErrorKind::Transient(reason.clone()),
                    retry_count,
                ) =>
            {
                if first_error_context.is_none() {
                    first_error_context = Some(format!(
                        "summary={} stderr={} termination_reason={:?}",
                        executed.execution.summary,
                        executed.execution.stderr_output,
                        session_state
                            .as_ref()
                            .and_then(|s| s.termination_reason.as_deref())
                    ));
                }
                retry_count += 1;
                warn!("Retrying debate after transient error: {reason}");
                continue;
            }
            DebateErrorKind::Transient(reason) => {
                if let Some(first) = first_error_context.as_deref() {
                    warn!(
                        first_error = first,
                        "Debate transient failure persisted after retry"
                    );
                }
                warn!("Debate ended after transient failure: {reason}");
                break executed;
            }
            DebateErrorKind::Deterministic(reason) => {
                debug!("Debate finished with deterministic non-zero outcome: {reason}");
                break executed;
            }
        }
    };

    let output = render_debate_output(
        &execution.execution.output,
        &execution.meta_session_id,
        execution.provider_session_id.as_deref(),
    );

    let debate_summary =
        extract_debate_summary(&output, execution.execution.summary.as_str(), debate_mode);
    let session_dir = csa_session::get_session_dir(&project_root, &execution.meta_session_id)?;
    let artifacts = persist_debate_output_artifacts(&session_dir, &debate_summary, &output)?;
    append_debate_artifacts_to_result(&project_root, &execution.meta_session_id, &artifacts)?;

    // 10. Print brief summary only.
    println!("{}", format_debate_stdout_summary(&debate_summary));

    Ok(execution.execution.exit_code)
}

const STILL_WORKING_BACKOFF: Duration = Duration::from_secs(5);

fn resolve_debate_tool(
    arg_tool: Option<ToolName>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    parent_tool: Option<&str>,
    project_root: &Path,
    force_override_user_config: bool,
) -> Result<(ToolName, DebateMode)> {
    // CLI --tool override always wins (explicit tool = heterogeneous intent)
    if let Some(tool) = arg_tool {
        // Enforce tool enablement when user explicitly selects a tool
        if let Some(cfg) = project_config {
            cfg.enforce_tool_enabled(tool.as_str(), force_override_user_config)?;
        }
        return Ok((tool, DebateMode::Heterogeneous));
    }

    // Project-level [debate] config override
    if let Some(project_debate) = project_config.and_then(|cfg| cfg.debate.as_ref()) {
        return resolve_debate_tool_from_value(
            &project_debate.tool,
            parent_tool,
            project_config,
            global_config,
            project_root,
        )
        .with_context(|| {
            format!(
                "Failed to resolve debate tool from project config: {}",
                ProjectConfig::config_path(project_root).display()
            )
        });
    }

    // When global [debate].tool is "auto", try priority-aware selection first
    if global_config.debate.tool == "auto" {
        let has_known_priority =
            csa_config::global::effective_tool_priority(project_config, global_config)
                .iter()
                .any(|entry| {
                    csa_config::global::all_known_tools()
                        .iter()
                        .any(|tool| tool.as_str() == entry)
                });
        if has_known_priority {
            if let Some(tool) = select_auto_debate_tool(parent_tool, project_config, global_config)
            {
                return Ok((tool, DebateMode::Heterogeneous));
            }
        }
    }

    // Global config [debate] section
    match global_config.resolve_debate_tool(parent_tool) {
        Ok(tool_name) => {
            // Skip disabled tools from global auto-resolution
            if let Some(cfg) = project_config {
                if !cfg.is_tool_enabled(&tool_name) {
                    // Try same-model fallback before giving up
                    return resolve_same_model_fallback(
                        parent_tool,
                        project_config,
                        global_config,
                        project_root,
                    );
                }
            }
            let tool = crate::run_helpers::parse_tool_name(&tool_name).map_err(|_| {
                anyhow::anyhow!(
                    "Invalid [debate].tool value '{}'. Supported values: gemini-cli, opencode, codex, claude-code.",
                    tool_name
                )
            })?;
            Ok((tool, DebateMode::Heterogeneous))
        }
        Err(_) => {
            // Heterogeneous selection failed — try same-model fallback
            resolve_same_model_fallback(parent_tool, project_config, global_config, project_root)
        }
    }
}

fn resolve_debate_tool_from_value(
    tool_value: &str,
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    project_root: &Path,
) -> Result<(ToolName, DebateMode)> {
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
            if let Some(tool) = select_auto_debate_tool(parent_tool, project_config, global_config)
            {
                return Ok((tool, DebateMode::Heterogeneous));
            }
        }

        // Try old heterogeneous_counterpart first for backward compatibility,
        // but only if the counterpart tool is enabled.
        if let Some(resolved) = parent_tool.and_then(heterogeneous_counterpart) {
            let counterpart_enabled =
                project_config.is_none_or(|cfg| cfg.is_tool_enabled(resolved));
            if counterpart_enabled {
                let tool = crate::run_helpers::parse_tool_name(resolved).map_err(|_| {
                    anyhow::anyhow!(
                        "BUG: auto debate tool resolution returned invalid tool '{}'",
                        resolved
                    )
                })?;
                return Ok((tool, DebateMode::Heterogeneous));
            }
        }

        // Fallback to ModelFamily-based selection (filtered by enabled tools)
        if let Some(tool) = select_auto_debate_tool(parent_tool, project_config, global_config) {
            return Ok((tool, DebateMode::Heterogeneous));
        }

        // All heterogeneous methods failed — try same-model fallback
        return resolve_same_model_fallback(
            parent_tool,
            project_config,
            global_config,
            project_root,
        );
    }

    let tool = crate::run_helpers::parse_tool_name(tool_value).map_err(|_| {
        anyhow::anyhow!(
            "Invalid project [debate].tool value '{}'. Supported values: auto, gemini-cli, opencode, codex, claude-code.",
            tool_value
        )
    })?;
    Ok((tool, DebateMode::Heterogeneous))
}

fn select_auto_debate_tool(
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

/// Attempt same-model adversarial fallback when heterogeneous selection fails.
///
/// Uses the parent tool (or any available tool) to run two independent sub-agents
/// as Proposer and Critic. Returns `SameModelAdversarial` mode to annotate output.
///
/// Fails with the standard auto-resolution error when:
/// - `same_model_fallback` is disabled in config
/// - No parent tool is detected and no tools are available
fn resolve_same_model_fallback(
    parent_tool: Option<&str>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    project_root: &Path,
) -> Result<(ToolName, DebateMode)> {
    if !global_config.debate.same_model_fallback {
        return Err(debate_auto_resolution_error(parent_tool, project_root));
    }

    // Use the parent tool itself for same-model adversarial debate,
    // but only if the tool is enabled in project config.
    if let Some(parent_str) = parent_tool {
        if let Ok(tool) = crate::run_helpers::parse_tool_name(parent_str) {
            let enabled = project_config
                .map(|cfg| cfg.is_tool_enabled(tool.as_str()))
                .unwrap_or(true);
            if enabled {
                return Ok((tool, DebateMode::SameModelAdversarial));
            }
        }
    }

    // No usable parent tool — select the first enabled tool from project config
    let candidates: Vec<_> = if let Some(cfg) = project_config {
        csa_config::global::all_known_tools()
            .iter()
            .filter(|t| cfg.is_tool_enabled(t.as_str()))
            .copied()
            .collect()
    } else {
        csa_config::global::all_known_tools().to_vec()
    };
    // Prefer a tool that is both enabled AND installed on this system.
    // Fall back to first enabled tool if none are installed (preserves prior behavior).
    let installed = candidates
        .iter()
        .find(|t| crate::run_helpers::is_tool_binary_available(t.as_str()));
    if let Some(tool) = installed.or(candidates.first()) {
        return Ok((*tool, DebateMode::SameModelAdversarial));
    }

    Err(debate_auto_resolution_error(parent_tool, project_root))
}

fn debate_auto_resolution_error(parent_tool: Option<&str>, project_root: &Path) -> anyhow::Error {
    let parent = parent_tool.unwrap_or("<none>").escape_default().to_string();
    let global_path = GlobalConfig::config_path()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.config/cli-sub-agent/config.toml".to_string());
    let project_path = ProjectConfig::config_path(project_root)
        .display()
        .to_string();

    anyhow::anyhow!(
        "AUTO debate tool selection failed (tool = \"auto\").\n\n\
STOP: Do not proceed. Ask the user to configure the debate tool explicitly.\n\n\
Parent tool context: {parent}\n\
Supported auto mapping: claude-code <-> codex\n\n\
Choose one:\n\
1) Global config (user-level): {global_path}\n\
   [debate]\n\
   tool = \"codex\"  # or \"claude-code\", \"opencode\", \"gemini-cli\"\n\
2) Project config override: {project_path}\n\
   [debate]\n\
   tool = \"codex\"  # or \"claude-code\", \"opencode\", \"gemini-cli\"\n\
3) CLI override: csa debate --tool codex\n\n\
Reason: CSA enforces heterogeneity in auto mode and will not fall back."
    )
}

/// Verify the debate pattern is installed before attempting execution.
///
/// Fails fast with actionable install guidance if the pattern is missing,
/// preventing silent degradation where the tool runs without skill context.
fn verify_debate_skill_available(project_root: &Path) -> Result<()> {
    match crate::pattern_resolver::resolve_pattern("debate", project_root) {
        Ok(resolved) => {
            debug!(
                pattern_dir = %resolved.dir.display(),
                has_config = resolved.config.is_some(),
                skill_md_len = resolved.skill_md.len(),
                "Debate pattern resolved"
            );
            Ok(())
        }
        Err(resolve_err) => {
            anyhow::bail!(
                "Debate pattern not found — `csa debate` requires the 'debate' pattern.\n\n\
                 {resolve_err}\n\n\
                 Install the debate pattern with one of:\n\
                 1) csa skill install RyderFreeman4Logos/cli-sub-agent\n\
                 2) Manually place skills/debate/SKILL.md (or PATTERN.md) inside .csa/patterns/debate/ or patterns/debate/\n\n\
                 Without the pattern, the debate tool cannot follow the structured debate protocol."
            )
        }
    }
}

/// Resolve stream mode for debate command.
///
/// - `--stream-stdout` forces TeeToStderr (progressive output)
/// - `--no-stream-stdout` forces BufferOnly (silent until complete)
/// - Default: auto-detect TTY on stderr -> TeeToStderr if interactive,
///   BufferOnly otherwise. Symmetric with review's behavior (#139).
fn resolve_debate_stream_mode(
    stream_stdout: bool,
    no_stream_stdout: bool,
) -> csa_process::StreamMode {
    if no_stream_stdout {
        csa_process::StreamMode::BufferOnly
    } else if stream_stdout || std::io::stderr().is_terminal() {
        csa_process::StreamMode::TeeToStderr
    } else {
        csa_process::StreamMode::BufferOnly
    }
}

fn resolve_debate_thinking(
    cli_thinking: Option<&str>,
    config_thinking: Option<&str>,
) -> Option<String> {
    cli_thinking
        .map(str::to_string)
        .or_else(|| config_thinking.map(str::to_string))
}

fn resolve_debate_timeout_seconds(
    cli_timeout_seconds: Option<u64>,
    global_timeout_seconds: Option<u64>,
) -> Option<u64> {
    cli_timeout_seconds.or(global_timeout_seconds)
}

fn ensure_debate_wall_clock_within_timeout(
    wall_clock_start: Instant,
    timeout_seconds: Option<u64>,
) -> Result<()> {
    if let Some(timeout_secs) = timeout_seconds
        && wall_clock_start.elapsed() > Duration::from_secs(timeout_secs)
    {
        anyhow::bail!("Wall-clock timeout exceeded ({timeout_secs}s)");
    }
    Ok(())
}

fn should_retry_debate_after_error(kind: &DebateErrorKind, retry_count: u8) -> bool {
    matches!(kind, DebateErrorKind::Transient(_)) && retry_count < 1
}

async fn wait_for_still_working_backoff() {
    tracing::info!("Tool still working, waiting before next attempt...");
    tokio::time::sleep(STILL_WORKING_BACKOFF).await;
}

/// Anti-recursion preamble injected into debate subprocess prompts.
///
/// Same guard as `review_cmd::ANTI_RECURSION_PREAMBLE` — prevents the spawned
/// tool from reading CLAUDE.md and recursively invoking CSA commands.
const ANTI_RECURSION_PREAMBLE: &str = "\
CRITICAL: You are running INSIDE a CSA subprocess (csa review / csa debate). \
Do NOT invoke `csa run`, `csa review`, `csa debate`, or ANY `csa` CLI command — \
this causes infinite recursion. Perform the task DIRECTLY using your own \
capabilities (Read, Grep, Glob, Bash for git commands). \
Ignore any CLAUDE.md or AGENTS.md rules that instruct you to delegate to CSA.\n\n";

/// Build a debate instruction that passes parameters to the debate skill.
///
/// The debate tool loads the debate skill from the project's `.claude/skills/`
/// directory and follows its instructions autonomously. We only pass parameters.
/// An anti-recursion preamble is prepended (see GitHub issue #272).
fn build_debate_instruction(question: &str, is_continuation: bool, rounds: u32) -> String {
    if is_continuation {
        format!(
            "{ANTI_RECURSION_PREAMBLE}Use the debate skill. continuation=true. rounds={rounds}. question={question}"
        )
    } else {
        format!(
            "{ANTI_RECURSION_PREAMBLE}Use the debate skill. rounds={rounds}. question={question}"
        )
    }
}

#[cfg(test)]
#[path = "debate_cmd_tests.rs"]
mod tests;
