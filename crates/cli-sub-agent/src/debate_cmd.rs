use std::io::IsTerminal;

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, error, warn};

use crate::cli::DebateArgs;
use crate::run_helpers::read_prompt;
use csa_config::global::{heterogeneous_counterpart, select_heterogeneous_tool};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;

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
    let tool = resolve_debate_tool(
        args.tool,
        config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
        &project_root,
    )?;

    // 6. Build executor and validate tool
    let executor = crate::pipeline::build_and_validate_executor(
        &tool,
        None,
        args.model.as_deref(),
        None,
        config.as_ref(),
        false, // skip tier whitelist for debate tool selection
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

    // 9. Execute with session (with optional absolute timeout)
    let description = format!(
        "debate: {}",
        crate::run_helpers::truncate_prompt(&question, 80)
    );
    let execute_future = crate::pipeline::execute_with_session_and_meta(
        &executor,
        &tool,
        &prompt,
        args.session,
        Some(description),
        None,
        &project_root,
        config.as_ref(),
        extra_env,
        Some("debate"),
        None, // debate does not use tier-based selection
        None, // debate does not override context loading options
        stream_mode,
        idle_timeout_seconds,
        Some(&global_config),
    );

    let timeout_secs = resolve_debate_timeout_seconds(args.timeout, &global_config);
    let execution =
        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), execute_future)
            .await
        {
            Ok(inner) => inner?,
            Err(_) => {
                error!(
                    timeout_secs = timeout_secs,
                    "Debate aborted: wall-clock timeout exceeded"
                );
                anyhow::bail!(
                    "Debate aborted: timeout {timeout_secs}s exceeded. \
                     Use --timeout to override, or set [debate].timeout_seconds in global config."
                );
            }
        };

    let output = render_debate_output(
        &execution.execution.output,
        &execution.meta_session_id,
        execution.provider_session_id.as_deref(),
    );

    // 10. Print result
    print!("{output}");

    Ok(execution.execution.exit_code)
}

fn resolve_debate_tool(
    arg_tool: Option<ToolName>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    parent_tool: Option<&str>,
    project_root: &Path,
) -> Result<ToolName> {
    // CLI --tool override always wins
    if let Some(tool) = arg_tool {
        return Ok(tool);
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
                return Ok(tool);
            }
        }
    }

    // Global config [debate] section
    match global_config.resolve_debate_tool(parent_tool) {
        Ok(tool_name) => crate::run_helpers::parse_tool_name(&tool_name).map_err(|_| {
            anyhow::anyhow!(
                "Invalid [debate].tool value '{}'. Supported values: gemini-cli, opencode, codex, claude-code.",
                tool_name
            )
        }),
        Err(_) => Err(debate_auto_resolution_error(parent_tool, project_root)),
    }
}

fn resolve_debate_tool_from_value(
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
            if let Some(tool) = select_auto_debate_tool(parent_tool, project_config, global_config)
            {
                return Ok(tool);
            }
        }

        // Try old heterogeneous_counterpart first for backward compatibility
        if let Some(resolved) = parent_tool.and_then(heterogeneous_counterpart) {
            return crate::run_helpers::parse_tool_name(resolved).map_err(|_| {
                anyhow::anyhow!(
                    "BUG: auto debate tool resolution returned invalid tool '{}'",
                    resolved
                )
            });
        }

        // Fallback to ModelFamily-based selection (filtered by enabled tools)
        if let Some(tool) = select_auto_debate_tool(parent_tool, project_config, global_config) {
            return Ok(tool);
        }

        // Both methods failed
        return Err(debate_auto_resolution_error(parent_tool, project_root));
    }

    crate::run_helpers::parse_tool_name(tool_value).map_err(|_| {
        anyhow::anyhow!(
            "Invalid project [debate].tool value '{}'. Supported values: auto, gemini-cli, opencode, codex, claude-code.",
            tool_value
        )
    })
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
                 2) Manually place skills/debate/SKILL.md inside .csa/patterns/debate/ or patterns/debate/\n\n\
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

fn resolve_debate_timeout_seconds(
    timeout_override: Option<u64>,
    global_config: &GlobalConfig,
) -> u64 {
    if let Some(timeout) = timeout_override {
        return timeout;
    }

    let configured_timeout = global_config.debate.timeout_seconds;
    if configured_timeout == 0 {
        let fallback_timeout = GlobalConfig::default().debate.timeout_seconds;
        warn!(
            fallback_timeout = fallback_timeout,
            "Invalid [debate].timeout_seconds=0 detected in global config; using default timeout"
        );
        return fallback_timeout;
    }

    configured_timeout
}

/// Build a debate instruction that passes parameters to the debate skill.
///
/// The debate tool loads the debate skill from the project's `.claude/skills/`
/// directory and follows its instructions autonomously. We only pass parameters.
fn build_debate_instruction(question: &str, is_continuation: bool, rounds: u32) -> String {
    if is_continuation {
        format!("Use the debate skill. continuation=true. rounds={rounds}. question={question}")
    } else {
        format!("Use the debate skill. rounds={rounds}. question={question}")
    }
}

fn render_debate_output(
    tool_output: &str,
    meta_session_id: &str,
    provider_session_id: Option<&str>,
) -> String {
    let mut output = match provider_session_id {
        Some(provider_id) => tool_output.replace(provider_id, meta_session_id),
        None => tool_output.to_string(),
    };

    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }

    output.push_str(&format!("CSA Meta Session ID: {meta_session_id}\n"));
    output
}

#[cfg(test)]
#[path = "debate_cmd_tests.rs"]
mod tests;
