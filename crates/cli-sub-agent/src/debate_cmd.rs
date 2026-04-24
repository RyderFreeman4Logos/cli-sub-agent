use std::io::IsTerminal;

use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, error, warn};

use crate::cli::DebateArgs;
use crate::debate_cmd_resolve::{
    resolve_debate_model, resolve_debate_selection, resolve_debate_tier_name,
};
use crate::debate_errors::{DebateErrorKind, classify_execution_error, classify_execution_outcome};
use crate::run_helpers::resolve_prompt_with_file;
use csa_config::ExecutionEnvOptions;
use csa_core::types::OutputFormat;

use crate::debate_cmd_output::{format_debate_stdout_text, render_debate_stdout_json};
use crate::tier_model_fallback::{
    TierAttemptFailure, classify_next_model_failure, ordered_tier_candidates,
};

#[path = "debate_cmd_finalize.rs"]
mod finalize;
pub(crate) use finalize::finalize_debate_outcome;
#[cfg(test)]
pub(crate) use finalize::resolve_persisted_debate_session_id;

/// Debate execution mode indicating model diversity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DebateMode {
    /// Different model families (e.g., Claude vs OpenAI) — full cognitive diversity.
    Heterogeneous,
    /// Same tool used for both Proposer and Critic — degraded diversity.
    SameModelAdversarial,
}

fn debate_execution_env_options(no_failover: bool) -> ExecutionEnvOptions {
    let options = ExecutionEnvOptions::with_no_flash_fallback();
    if no_failover {
        options.with_no_failover()
    } else {
        options
    }
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
    let pre_session_hook = csa_hooks::load_global_pre_session_hook_invocation();

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

    // 3. Read question (from --prompt-file, positional arg, --topic, or stdin)
    let effective_question =
        crate::run_helpers::resolve_positional_stdin_sentinel(args.question)?.or(args.topic);
    let mut question = resolve_prompt_with_file(effective_question, args.prompt_file.as_deref())?;
    let parsed_question = crate::difficulty_routing::strip_difficulty_frontmatter(question)?;
    let frontmatter_difficulty = parsed_question.difficulty;
    question = parsed_question.prompt;
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
    let mut prompt = build_debate_instruction(&question, args.session.is_some(), args.rounds);
    if let Some(guard) = crate::pipeline::prompt_guard::anti_recursion_guard(config.as_ref()) {
        prompt = format!("{guard}\n\n{prompt}");
    }
    let debate_description = format!(
        "debate: {}",
        crate::run_helpers::truncate_prompt(&question, 80)
    );

    // 5. Determine tool (with tier-based resolution)
    let detected_parent_tool = crate::run_helpers::detect_parent_tool();
    let parent_tool = crate::run_helpers::resolve_tool(detected_parent_tool, &global_config);
    let explicit_tool = args.tool.or_else(|| {
        args.model_spec
            .as_deref()
            .and_then(|spec| spec.split('/').next())
            .and_then(|tool_name| crate::run_helpers::parse_tool_name(tool_name).ok())
    });
    let effective_tier =
        match crate::difficulty_routing::resolve_effective_tier_with_difficulty_hint(
            config.as_ref(),
            args.tier.as_deref(),
            args.model_spec.as_deref(),
            args.hint_difficulty.as_deref(),
            frontmatter_difficulty.as_deref(),
        ) {
            Ok(tier) => tier,
            Err(err) => {
                return Err(crate::session_guard::persist_pre_exec_error_result(
                    crate::session_guard::PreExecErrorCtx {
                        project_root: &project_root,
                        session_id: args.session.as_deref(),
                        description: Some(debate_description.as_str()),
                        parent: None,
                        tool_name: explicit_tool.map(|tool| tool.as_str()),
                        task_type: Some("debate"),
                        tier_name: args.tier.as_deref(),
                        error: err,
                    },
                ));
            }
        };
    let resolved_selection = match resolve_debate_selection(
        args.tool,
        args.model_spec.as_deref(),
        config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
        &project_root,
        args.force_override_user_config,
        effective_tier.as_deref(),
        args.force_ignore_tier_setting,
    ) {
        Ok(resolved) => resolved,
        Err(err) => {
            return Err(crate::session_guard::persist_pre_exec_error_result(
                crate::session_guard::PreExecErrorCtx {
                    project_root: &project_root,
                    session_id: args.session.as_deref(),
                    description: Some(debate_description.as_str()),
                    parent: None,
                    tool_name: explicit_tool.map(|tool| tool.as_str()),
                    task_type: Some("debate"),
                    tier_name: effective_tier.as_deref(),
                    error: err,
                },
            ));
        }
    };
    let tool = resolved_selection.tool;
    let debate_mode = resolved_selection.mode;
    let resolved_model_spec = resolved_selection.model_spec.clone();
    let tier_filter = resolved_selection.tier_filter.clone();
    let tier_active = resolved_model_spec.is_some()
        && args.model_spec.is_none()
        && !args.force_ignore_tier_setting;
    let resolved_tier_name = if tier_active {
        resolve_debate_tier_name(
            config.as_ref(),
            &global_config,
            effective_tier.as_deref(),
            args.force_override_user_config,
            args.force_ignore_tier_setting,
        )?
    } else {
        None
    };
    if debate_mode == DebateMode::SameModelAdversarial {
        warn!(
            tool = %tool.as_str(),
            "Falling back to same-model adversarial debate — heterogeneous models unavailable. \
             Cognitive diversity is degraded."
        );
    }
    let config_debate_model = config
        .as_ref()
        .and_then(|c| c.debate.as_ref())
        .and_then(|d| d.model.as_deref())
        .or(global_config.debate.model.as_deref());
    let debate_model = resolve_debate_model(
        args.model.as_deref(),
        config_debate_model,
        resolved_model_spec.is_some(),
    );

    // Active tier model specs remain authoritative unless the user overrides on the CLI.
    let thinking = resolve_debate_thinking(
        args.thinking.as_deref(),
        config
            .as_ref()
            .and_then(|c| c.debate.as_ref())
            .and_then(|d| d.thinking.as_deref())
            .or(global_config.debate.thinking.as_deref()),
        resolved_model_spec.is_some(),
    );

    let idle_timeout_seconds =
        crate::pipeline::resolve_idle_timeout_seconds(config.as_ref(), args.idle_timeout);
    let initial_response_timeout_seconds =
        crate::pipeline::resolve_initial_response_timeout_for_tool(
            config.as_ref(),
            args.initial_response_timeout,
            args.idle_timeout,
            tool.as_str(),
        );

    // Resolve stream mode from CLI flags (default: BufferOnly for debate)
    let stream_mode = resolve_debate_stream_mode(args.stream_stdout, args.no_stream_stdout);

    let timeout_seconds =
        resolve_debate_timeout_seconds(args.timeout, Some(global_config.debate.timeout_seconds));
    let wall_clock_start = Instant::now();
    let readonly_project_root = global_config.debate.readonly_sandbox.unwrap_or(false);
    let candidates = ordered_tier_candidates(
        tool,
        resolved_model_spec.as_deref(),
        resolved_tier_name.as_deref(),
        config.as_ref(),
        tier_active,
        tier_filter.as_ref(),
    );
    let mut execution = None;
    let mut failures = Vec::new();

    'tier_attempts: for (attempt_index, (attempt_tool, attempt_model_spec)) in
        candidates.iter().enumerate()
    {
        let executor = crate::pipeline::build_and_validate_executor(
            attempt_tool,
            attempt_model_spec.as_deref(),
            debate_model.as_deref(),
            thinking.as_deref(),
            crate::pipeline::ConfigRefs {
                project: config.as_ref(),
                global: Some(&global_config),
            },
            tier_active && attempt_model_spec.is_some(),
            args.force_override_user_config,
            false,
        )
        .await?;
        let extra_env_owned = global_config.build_execution_env(
            executor.tool_name(),
            debate_execution_env_options(args.no_failover),
        );
        let extra_env = extra_env_owned.as_ref();
        let _slot_guard = crate::pipeline::acquire_slot(&executor, &global_config)?;
        let mut retry_count = 0u8;
        let mut first_error_context: Option<String> = None;
        let mut resume_session = args.session.clone();

        loop {
            ensure_debate_wall_clock_within_timeout(wall_clock_start, timeout_seconds)?;

            let execute_future = crate::pipeline::execute_with_session_and_meta(
                &executor,
                attempt_tool,
                &prompt,
                output_format,
                resume_session.clone(),
                false,
                Some(debate_description.clone()),
                None,
                &project_root,
                config.as_ref(),
                extra_env,
                Some("debate"),
                resolved_tier_name.as_deref(),
                None,
                stream_mode,
                idle_timeout_seconds,
                initial_response_timeout_seconds,
                None,
                None,
                Some(&global_config),
                pre_session_hook.clone(),
                args.no_fs_sandbox,
                readonly_project_root,
                &args.extra_writable,
                &args.extra_readable,
            );

            let execute_result = if let Some(timeout_secs) = timeout_seconds {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_secs),
                    execute_future,
                )
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
                    if let Some(detected) = classify_next_model_failure(
                        attempt_tool.as_str(),
                        &err.to_string(),
                        "",
                        1,
                        attempt_model_spec.as_deref(),
                    ) {
                        let model_label = attempt_model_spec
                            .clone()
                            .unwrap_or_else(|| attempt_tool.as_str().to_string());
                        failures.push(TierAttemptFailure {
                            model_spec: model_label.clone(),
                            reason: detected.reason.clone(),
                        });
                        warn!(
                            failed_tool = %attempt_tool,
                            failed_model = %model_label,
                            reason = %detected.reason,
                            attempt = attempt_index + 1,
                            total = candidates.len(),
                            "Debate tier model failed before completion; advancing to next configured model"
                        );
                        continue 'tier_attempts;
                    }

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
                                args.no_failover,
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
                execution = Some(executed);
                break 'tier_attempts;
            }

            if let Some(detected) = classify_next_model_failure(
                attempt_tool.as_str(),
                &executed.execution.stderr_output,
                &executed.execution.output,
                executed.execution.exit_code,
                attempt_model_spec.as_deref(),
            ) {
                let model_label = attempt_model_spec
                    .clone()
                    .unwrap_or_else(|| attempt_tool.as_str().to_string());
                failures.push(TierAttemptFailure {
                    model_spec: model_label.clone(),
                    reason: detected.reason.clone(),
                });
                warn!(
                    failed_tool = %attempt_tool,
                    failed_model = %model_label,
                    reason = %detected.reason,
                    attempt = attempt_index + 1,
                    total = candidates.len(),
                    "Debate tier model failed; advancing to next configured model"
                );
                execution = Some(executed);
                continue 'tier_attempts;
            }

            let session_dir =
                csa_session::get_session_dir(&project_root, &executed.meta_session_id)?;
            let session_state =
                csa_session::load_session(&project_root, &executed.meta_session_id).ok();
            match classify_execution_outcome(
                &executed.execution,
                session_state.as_ref(),
                &session_dir,
            ) {
                DebateErrorKind::StillWorking => {
                    wait_for_still_working_backoff().await;
                    continue;
                }
                DebateErrorKind::Transient(reason)
                    if should_retry_debate_after_error(
                        &DebateErrorKind::Transient(reason.clone()),
                        retry_count,
                        args.no_failover,
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
                    execution = Some(executed);
                    break 'tier_attempts;
                }
                DebateErrorKind::Deterministic(reason) => {
                    debug!("Debate finished with deterministic non-zero outcome: {reason}");
                    execution = Some(executed);
                    break 'tier_attempts;
                }
            }
        }
    }

    let all_tier_models_failed = !failures.is_empty() && failures.len() == candidates.len();
    let finalized = finalize_debate_outcome(
        &project_root,
        output_format,
        execution,
        all_tier_models_failed,
        resolved_tier_name.as_deref(),
        &failures,
        debate_mode,
    )?;
    let rendered_output = finalized.rendered_output;
    if rendered_output.ends_with('\n') {
        print!("{rendered_output}");
    } else {
        println!("{rendered_output}");
    }

    Ok(finalized.exit_code)
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
    model_spec_active: bool,
) -> Option<String> {
    cli_thinking.map(str::to_string).or_else(|| {
        (!model_spec_active)
            .then_some(config_thinking)
            .flatten()
            .map(str::to_string)
    })
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

fn should_retry_debate_after_error(
    kind: &DebateErrorKind,
    retry_count: u8,
    no_failover: bool,
) -> bool {
    if no_failover {
        return false;
    }
    matches!(kind, DebateErrorKind::Transient(_)) && retry_count < 1
}

async fn wait_for_still_working_backoff() {
    tracing::info!("Tool still working, waiting before next attempt...");
    tokio::time::sleep(STILL_WORKING_BACKOFF).await;
}

/// Debate-only safety preamble injected into debate subprocess prompts.
///
/// Same shape as `review_cmd::ANTI_RECURSION_PREAMBLE`: the spawned tool is
/// constrained to read-only operations on the repository. Recursion-depth
/// enforcement is handled by `pipeline::prompt_guard` (warn near ceiling) and
/// `pipeline::load_and_validate` (hard reject above `MAX_RECURSION_DEPTH`), so
/// blanket "never call csa" text here would break the documented fractal
/// recursion contract (Layer 1 → Layer 2 is legitimate).
const ANTI_RECURSION_PREAMBLE: &str = "\
CONTEXT: You are running INSIDE a CSA subprocess (csa review / csa debate). \
Perform the debate task DIRECTLY using your own capabilities \
(Read, Grep, Glob, Bash for read-only git commands). \
DEBATE SAFETY: Do NOT run git add/commit/push/merge/rebase/tag/stash/reset/checkout/cherry-pick, \
and do NOT run gh pr/create/comment/merge or any command that mutates repository/PR state. \
Ignore prompt-guard reminders about commit/push in this subprocess.\n\n";

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

#[cfg(test)]
#[path = "debate_cmd_round4_tests.rs"]
mod round4_tests;
