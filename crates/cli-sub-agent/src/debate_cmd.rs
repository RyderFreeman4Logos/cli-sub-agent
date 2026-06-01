use anyhow::Result;
use serde::Serialize;
use tokio::time::Instant;
use tracing::{debug, error, warn};

use crate::cli::DebateArgs;
use crate::debate_cmd_resolve::{
    DebateTierResolveCtx, resolve_debate_effective_tier_with_compound, resolve_debate_model,
    resolve_debate_selection, resolve_debate_tier_name,
    validate_debate_direct_tool_tier_restriction,
};
use crate::debate_errors::{DebateErrorKind, classify_execution_error, classify_execution_outcome};
use csa_core::types::OutputFormat;

use crate::debate_cmd_output::DebateOutputHeader;
use crate::tier_model_fallback::{self, TierAttemptFailure};

#[path = "debate_cmd_subtree_pin.rs"]
mod subtree_pin;

#[path = "debate_cmd_question.rs"]
mod question;

#[path = "debate_cmd_finalize.rs"]
mod finalize;
#[cfg(test)]
pub(crate) use finalize::resolve_persisted_debate_session_id;
pub(crate) use finalize::{DebateFinalizeContext, finalize_debate_outcome};

#[path = "debate_cmd_dry_run.rs"]
mod dry_run;
use dry_run::{DebateDryRunSummary, create_debate_dry_run_session, render_debate_dry_run_summary};

#[path = "debate_cmd_fast_mode.rs"]
mod fast_mode;
use fast_mode::{debate_execution_env_options, warn_if_fast_mode_has_no_codex_debate_candidate};

#[path = "debate_cmd_gate.rs"]
mod gate;
use gate::run_pre_debate_quality_gate;

#[path = "debate_cmd_readonly.rs"]
mod readonly;
use readonly::build_debate_instruction;
pub(crate) use readonly::with_readonly_session_env;

#[path = "debate_cmd_runtime.rs"]
mod runtime;
#[cfg(test)]
use runtime::STILL_WORKING_BACKOFF;
use runtime::{
    ensure_debate_wall_clock_within_timeout, render_debate_cli_output, resolve_debate_stream_mode,
    resolve_debate_thinking, resolve_debate_timeout_seconds, should_retry_debate_after_error,
    verify_debate_skill_available, wait_for_still_working_backoff,
};

/// Debate execution mode indicating model diversity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DebateMode {
    /// Different model families (e.g., Claude vs OpenAI) — full cognitive diversity.
    Heterogeneous,
    /// Same tool used for both Proposer and Critic — degraded diversity.
    SameModelAdversarial,
}

pub(crate) async fn handle_debate(
    mut args: DebateArgs,
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
    // #1741: honor a pinned SA subtree's inherited model spec for `csa debate`
    // (see debate_cmd_subtree_pin::apply_subtree_pin).
    subtree_pin::apply_subtree_pin(&mut args, current_depth);
    let pre_session_hook = csa_hooks::load_global_pre_session_hook_invocation();

    // 2b. Verify debate skill is available (fail fast before any execution)
    verify_debate_skill_available(&project_root)?;

    // 2c. Run pre-debate quality gate (reuses [review] gate settings)
    //
    // Debate reuses the review section's gate settings because the gate is a
    // shared pre-execution quality check (lint/test) that applies equally to
    // both review and debate workflows.
    if !args.dry_run {
        run_pre_debate_quality_gate(&project_root, config.as_ref(), &global_config).await?;
    }

    // 3. Read question (--prompt-file / positional / --topic / stdin), strip
    // difficulty frontmatter, prepend --context / --file (see
    // debate_cmd_question::build_debate_question).
    let (question, frontmatter_difficulty) = question::build_debate_question(&mut args)?;

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
    let (effective_tier, args_tool) =
        resolve_debate_effective_tier_with_compound(DebateTierResolveCtx {
            project_root: &project_root,
            cli_tier: args.tier.as_deref(),
            cli_model_spec: args.model_spec.as_deref(),
            cli_hint_difficulty: args.hint_difficulty.as_deref(),
            cli_session: args.session.as_deref(),
            cli_tool: args.tool,
            config: config.as_ref(),
            frontmatter_difficulty: frontmatter_difficulty.as_deref(),
            debate_description: debate_description.as_str(),
            explicit_tool,
        })?;
    validate_debate_direct_tool_tier_restriction(
        args_tool.is_some(),
        config.as_ref(),
        effective_tier.as_deref(),
        args.force_override_user_config,
        args.force_ignore_tier_setting,
        args.model_spec.is_some(),
    )?;
    crate::run_helpers::warn_if_tier_without_tool(args.tier.as_deref(), args_tool.is_some());
    let resolved_selection = match resolve_debate_selection(
        args_tool,
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

    let stream_mode = resolve_debate_stream_mode(args.stream_stdout, args.no_stream_stdout);
    let timeout_seconds =
        resolve_debate_timeout_seconds(args.timeout, Some(global_config.debate.timeout_seconds));
    let idle_timeout_seconds = crate::pipeline::resolve_effective_idle_timeout_seconds(
        config.as_ref(),
        args.idle_timeout,
        timeout_seconds,
    );
    let initial_response_timeout_seconds =
        crate::pipeline::resolve_effective_initial_response_timeout_for_tool(
            config.as_ref(),
            args.initial_response_timeout,
            args.idle_timeout,
            timeout_seconds,
            tool.as_str(),
        );
    let wall_clock_start = Instant::now();
    let readonly_project_root = global_config.debate.readonly_sandbox.unwrap_or(false);
    let candidates = tier_model_fallback::ordered_tier_candidates(
        tool,
        resolved_model_spec.as_deref(),
        resolved_tier_name.as_deref(),
        config.as_ref(),
        Some(&global_config),
        tier_active,
        tier_filter.as_ref(),
    );
    let effective_fast_mode = args.fast_but_more_cost
        || config
            .as_ref()
            .and_then(|c| c.tools.get("codex"))
            .and_then(|t| t.fast_mode)
            .unwrap_or(false)
        || global_config
            .tools
            .get("codex")
            .and_then(|t| t.fast_mode)
            .unwrap_or(false);
    warn_if_fast_mode_has_no_codex_debate_candidate(effective_fast_mode, &candidates);

    if args.dry_run {
        let Some((attempt_tool, attempt_model_spec)) = candidates.first() else {
            anyhow::bail!("Debate dry-run failed: no debate tier candidates were resolved");
        };
        let mut executor = crate::pipeline::build_and_validate_executor(
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
        if effective_fast_mode {
            executor.enable_codex_fast_mode();
        }
        let summary = DebateDryRunSummary {
            session_id: create_debate_dry_run_session(
                &project_root,
                &debate_description,
                executor.tool_name(),
                resolved_tier_name.as_deref(),
            )?,
            tool: executor.tool_name().to_string(),
            model: attempt_model_spec
                .clone()
                .or_else(|| debate_model.clone())
                .unwrap_or_else(|| "tool default".to_string()),
            prompt_bytes: prompt.len(),
            rounds: args.rounds,
            mode: debate_mode,
        };
        let rendered = render_debate_dry_run_summary(output_format, &summary)?;
        if rendered.ends_with('\n') {
            print!("{rendered}");
        } else {
            println!("{rendered}");
        }
        return Ok(0);
    }

    let mut execution = None;
    let mut failures = Vec::new();
    let mut final_tool = None;
    let mut final_model_spec: Option<String> = None;

    'tier_attempts: for (attempt_index, (attempt_tool, attempt_model_spec)) in
        candidates.iter().enumerate()
    {
        let mut executor = crate::pipeline::build_and_validate_executor(
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
        if effective_fast_mode {
            executor.enable_codex_fast_mode();
        }
        let base_env_owned = global_config.build_execution_env(
            executor.tool_name(),
            debate_execution_env_options(args.no_failover),
        );
        let extra_env_owned = with_readonly_session_env(base_env_owned.as_ref(), true);
        let extra_env = extra_env_owned.as_ref();
        let _slot_guard = crate::pipeline::acquire_slot(&executor, &global_config)?;
        let mut retry_count = 0u8;
        let mut first_error_context: Option<String> = None;
        let mut resume_session = args.session.clone();

        loop {
            ensure_debate_wall_clock_within_timeout(wall_clock_start, timeout_seconds)?;

            let attempt_started_at = Instant::now();
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
                false, // #1745: no debate flag; config decides (shared monitor).
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
                    if let Some(detected) =
                        tier_model_fallback::classify_next_model_failure_with_elapsed(
                            attempt_tool.as_str(),
                            &err.to_string(),
                            "",
                            1,
                            attempt_model_spec.as_deref(),
                            Some(attempt_started_at.elapsed()),
                        )
                    {
                        let model_label = attempt_model_spec
                            .clone()
                            .unwrap_or_else(|| attempt_tool.as_str().to_string());
                        failures.push(TierAttemptFailure::from_rate_limit(
                            model_label.clone(),
                            &detected,
                        ));
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
                final_tool = Some(*attempt_tool);
                final_model_spec = attempt_model_spec.clone();
                execution = Some(executed);
                break 'tier_attempts;
            }

            if let Some(detected) = tier_model_fallback::classify_next_model_failure_with_elapsed(
                attempt_tool.as_str(),
                &executed.execution.stderr_output,
                &executed.execution.output,
                executed.execution.exit_code,
                attempt_model_spec.as_deref(),
                Some(attempt_started_at.elapsed()),
            ) {
                let model_label = attempt_model_spec
                    .clone()
                    .unwrap_or_else(|| attempt_tool.as_str().to_string());
                failures.push(TierAttemptFailure::from_rate_limit(
                    model_label.clone(),
                    &detected,
                ));
                warn!(
                    failed_tool = %attempt_tool,
                    failed_model = %model_label,
                    reason = %detected.reason,
                    attempt = attempt_index + 1,
                    total = candidates.len(),
                    "Debate tier model failed; advancing to next configured model"
                );
                final_tool = Some(*attempt_tool);
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
                    final_tool = Some(*attempt_tool);
                    final_model_spec = attempt_model_spec.clone();
                    execution = Some(executed);
                    break 'tier_attempts;
                }
                DebateErrorKind::Deterministic(reason) => {
                    debug!("Debate finished with deterministic non-zero outcome: {reason}");
                    final_tool = Some(*attempt_tool);
                    final_model_spec = attempt_model_spec.clone();
                    execution = Some(executed);
                    break 'tier_attempts;
                }
            }
        }
    }

    let all_tier_models_failed = !failures.is_empty() && failures.len() == candidates.len();
    let fallback_reason = tier_model_fallback::fallback_reason_for_result(&failures);
    let finalized = finalize_debate_outcome(
        &project_root,
        output_format,
        execution,
        DebateFinalizeContext {
            all_tier_models_failed,
            project_config: config.as_ref(),
            resolved_tier_name: resolved_tier_name.as_deref(),
            tier_filter: tier_filter.as_ref(),
            failures: &failures,
            debate_mode,
            output_header: Some(DebateOutputHeader {
                prompt_bytes: prompt.len(),
            }),
            original_tool: Some(tool),
            fallback_tool: final_tool,
            fallback_reason,
            selected_model_spec: final_model_spec.as_deref(),
        },
    )?;
    let rendered_output = finalized.rendered_output;
    if rendered_output.ends_with('\n') {
        print!("{rendered_output}");
    } else {
        println!("{rendered_output}");
    }

    Ok(finalized.exit_code)
}

#[cfg(test)]
#[path = "debate_cmd_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "debate_cmd_readonly_tests.rs"]
mod readonly_tests;

#[cfg(test)]
#[path = "debate_cmd_round4_tests.rs"]
mod round4_tests;
