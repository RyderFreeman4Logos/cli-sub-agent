use std::io::IsTerminal;

use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, error, warn};

use crate::cli::DebateArgs;
use crate::debate_cmd_resolve::resolve_debate_tool;
use crate::debate_errors::{DebateErrorKind, classify_execution_error, classify_execution_outcome};
use crate::run_helpers::read_prompt;
use csa_config::ExecutionEnvOptions;
use csa_core::types::OutputFormat;

use crate::debate_cmd_output::{
    append_debate_artifacts_to_result, extract_debate_summary, format_debate_stdout_text,
    persist_debate_output_artifacts, render_debate_output, render_debate_stdout_json,
};

/// Debate execution mode indicating model diversity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DebateMode {
    /// Different model families (e.g., Claude vs OpenAI) — full cognitive diversity.
    Heterogeneous,
    /// Same tool used for both Proposer and Critic — degraded diversity.
    SameModelAdversarial,
}

pub(crate) async fn handle_debate(
    args: DebateArgs,
    current_depth: u32,
    output_format: OutputFormat,
) -> Result<i32> {
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

    // 2c. Run pre-debate quality gate (reuses [review] gate settings)
    //
    // Debate reuses the review section's gate settings because the gate is a
    // shared pre-execution quality check (lint/test) that applies equally to
    // both review and debate workflows.
    {
        let gate_steps = global_config.review.effective_gate_steps();
        let gate_timeout = config
            .as_ref()
            .and_then(|c| c.review.as_ref())
            .map(|r| r.gate_timeout_secs)
            .unwrap_or_else(csa_config::ReviewConfig::default_gate_timeout);
        let gate_mode = &global_config.review.gate_mode;

        if gate_steps.is_empty() {
            // Legacy single-command path
            let gate_command = config
                .as_ref()
                .and_then(|c| c.review.as_ref())
                .and_then(|r| r.gate_command.as_deref());
            let gate_result = crate::pipeline::gate::evaluate_quality_gate(
                &project_root,
                gate_command,
                gate_timeout,
                gate_mode,
            )
            .await?;

            if gate_result.skipped {
                debug!(
                    reason = gate_result.skip_reason.as_deref().unwrap_or("unknown"),
                    "Quality gate skipped"
                );
            } else if !gate_result.passed() {
                match gate_mode {
                    csa_config::GateMode::Monitor => {
                        warn!(
                            command = %gate_result.command,
                            exit_code = ?gate_result.exit_code,
                            "Quality gate failed (monitor mode — continuing with debate)"
                        );
                    }
                    csa_config::GateMode::CriticalOnly | csa_config::GateMode::Full => {
                        let mut msg = format!(
                            "Pre-debate quality gate failed (mode={gate_mode:?}).\n\
                             Command: {}\nExit code: {:?}",
                            gate_result.command, gate_result.exit_code
                        );
                        if !gate_result.stdout.is_empty() {
                            msg.push_str(&format!("\n--- stdout ---\n{}", gate_result.stdout));
                        }
                        if !gate_result.stderr.is_empty() {
                            msg.push_str(&format!("\n--- stderr ---\n{}", gate_result.stderr));
                        }
                        anyhow::bail!(msg);
                    }
                }
            } else {
                debug!(command = %gate_result.command, "Quality gate passed");
            }
        } else {
            // Multi-step pipeline
            let pipeline_result = crate::pipeline::gate::evaluate_quality_gates(
                &project_root,
                &gate_steps,
                gate_timeout,
                gate_mode,
            )
            .await?;

            if pipeline_result.passed {
                debug!("Quality gate pipeline passed");
            } else {
                match gate_mode {
                    csa_config::GateMode::Monitor => {
                        warn!("Quality gate pipeline failed (monitor mode — continuing)");
                    }
                    csa_config::GateMode::CriticalOnly | csa_config::GateMode::Full => {
                        let failed = pipeline_result.failed_step.as_deref().unwrap_or("unknown");
                        let mut msg = format!(
                            "Pre-debate quality gate pipeline FAILED at step: {failed}\n\
                             (mode={gate_mode:?})\n"
                        );
                        for step in &pipeline_result.steps {
                            if !step.passed() {
                                msg.push_str(&format!(
                                    "\nL{} {} ({}): exit {:?}",
                                    step.level, step.name, step.command, step.exit_code
                                ));
                                if !step.stderr.is_empty() {
                                    msg.push_str(&format!("\n  stderr: {}", step.stderr));
                                }
                            }
                        }
                        anyhow::bail!(msg);
                    }
                }
            }
        }
    }

    // 3. Read question (from positional arg, --topic, or stdin)
    let effective_question = args.question.or(args.topic);
    let mut question = read_prompt(effective_question)?;
    if let Some(ctx) = &args.context {
        question = format!("<debate-context>\n{ctx}\n</debate-context>\n\n{question}");
    }
    if let Some(file_path) = &args.file {
        const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024; // 5 MB
        let metadata = std::fs::metadata(file_path)
            .with_context(|| format!("Failed to stat --file: {file_path}"))?;
        if metadata.len() > MAX_FILE_SIZE {
            anyhow::bail!(
                "--file '{}' is too large ({} bytes, max {} bytes)",
                file_path,
                metadata.len(),
                MAX_FILE_SIZE
            );
        }
        let file_content = std::fs::read_to_string(file_path)
            .with_context(|| format!("Failed to read --file: {file_path}"))?;
        question = format!(
            "<attached-file path=\"{file_path}\">\n{file_content}\n</attached-file>\n\n{question}"
        );
    }

    // 4. Build debate instruction (parameter passing — tool loads debate skill)
    let prompt = build_debate_instruction(&question, args.session.is_some(), args.rounds);

    // 5. Determine tool (with tier-based resolution)
    let detected_parent_tool = crate::run_helpers::detect_parent_tool();
    let parent_tool = crate::run_helpers::resolve_tool(detected_parent_tool, &global_config);
    let (tool, debate_mode, tier_model_spec) = resolve_debate_tool(
        args.tool,
        config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
        &project_root,
        args.force_override_user_config,
        args.tier.as_deref(),
        args.force_ignore_tier_setting,
    )?;
    if debate_mode == DebateMode::SameModelAdversarial {
        warn!(
            tool = %tool.as_str(),
            "Falling back to same-model adversarial debate — heterogeneous models unavailable. \
             Cognitive diversity is degraded."
        );
    }
    // Model precedence: CLI --model > project config debate.model > global config debate.model.
    // When tier is also set, build_executor applies model override after tier spec construction.
    let debate_model = args.model.clone().or_else(|| {
        config
            .as_ref()
            .and_then(|c| c.debate.as_ref())
            .and_then(|d| d.model.clone())
            .or_else(|| global_config.debate.model.clone())
    });

    // Thinking precedence: CLI > config debate.thinking > tier model_spec thinking.
    let thinking = resolve_debate_thinking(
        args.thinking.as_deref(),
        config
            .as_ref()
            .and_then(|c| c.debate.as_ref())
            .and_then(|d| d.thinking.as_deref())
            .or(global_config.debate.thinking.as_deref()),
    );

    // 6. Build executor and validate tool
    let enforce_tier = tier_model_spec.is_some();
    let executor = crate::pipeline::build_and_validate_executor(
        &tool,
        tier_model_spec.as_deref(),
        debate_model.as_deref(),
        thinking.as_deref(),
        crate::pipeline::ConfigRefs {
            project: config.as_ref(),
            global: Some(&global_config),
        },
        enforce_tier,
        args.force_override_user_config,
        false, // debate must not inherit `csa run` per-tool defaults
    )
    .await?;

    // 7. Get env injection from global config (with no-flash + api key fallback)
    let extra_env_owned = global_config.build_execution_env(
        executor.tool_name(),
        ExecutionEnvOptions::with_no_flash_fallback(),
    );
    let extra_env = extra_env_owned.as_ref();
    let idle_timeout_seconds =
        crate::pipeline::resolve_idle_timeout_seconds(config.as_ref(), args.idle_timeout);
    let initial_response_timeout_seconds =
        crate::pipeline::resolve_initial_response_timeout_seconds(
            config.as_ref(),
            args.initial_response_timeout,
        );

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
    let readonly_project_root = global_config.debate.readonly_sandbox.unwrap_or(false);

    let execution = loop {
        ensure_debate_wall_clock_within_timeout(wall_clock_start, timeout_seconds)?;

        let execute_future = crate::pipeline::execute_with_session_and_meta(
            &executor,
            &tool,
            &prompt,
            output_format,
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
            initial_response_timeout_seconds,
            None,
            None,
            Some(&global_config),
            args.no_fs_sandbox,
            readonly_project_root,
            &args.extra_writable,
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

    let rendered_output = render_debate_cli_output(
        output_format,
        &debate_summary,
        &output,
        &execution.meta_session_id,
    )?;
    if rendered_output.ends_with('\n') {
        print!("{rendered_output}");
    } else {
        println!("{rendered_output}");
    }

    Ok(execution.execution.exit_code)
}

fn render_debate_cli_output(
    output_format: OutputFormat,
    debate_summary: &crate::debate_cmd_output::DebateSummary,
    transcript: &str,
    meta_session_id: &str,
) -> Result<String> {
    match output_format {
        OutputFormat::Text => Ok(format_debate_stdout_text(debate_summary, transcript)),
        OutputFormat::Json => {
            render_debate_stdout_json(debate_summary, transcript, meta_session_id)
        }
    }
}

const STILL_WORKING_BACKOFF: Duration = Duration::from_secs(5);

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
capabilities (Read, Grep, Glob, Bash for read-only git commands). \
DEBATE SAFETY: Do NOT run git add/commit/push/merge/rebase/tag/stash/reset/checkout/cherry-pick, \
and do NOT run gh pr/create/comment/merge or any command that mutates repository/PR state. \
Ignore prompt-guard reminders about commit/push in this subprocess. \
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
