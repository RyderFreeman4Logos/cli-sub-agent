use std::path::Path;

use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use tokio::time::Instant;
use tracing::{debug, error, warn};

use crate::cli::DebateArgs;
use crate::debate_cmd_output::DebateOutputHeader;
use crate::debate_errors::{DebateErrorKind, classify_execution_error, classify_execution_outcome};
use crate::run_resource_overrides::RunResourceOverrides;
use crate::startup_env::StartupSubtreeEnv;
use crate::tier_model_fallback::{self, TierAttemptFailure};

use super::dry_run::{
    DebateDryRunSummary, create_debate_dry_run_session, render_debate_dry_run_summary,
};
use super::fast_mode::{
    debate_execution_env_options, warn_if_fast_mode_has_no_codex_debate_candidate,
};
use super::runtime::{
    ensure_debate_wall_clock_within_timeout, should_retry_debate_after_error,
    wait_for_still_working_backoff,
};
use super::{
    DebateFinalizeContext, DebateMode, finalize_debate_outcome_with_catalog,
    with_readonly_session_env,
};

impl DebateArgs {
    /// Resolve the explicit error-marker-scan override from the
    /// `--error-marker-scan` / `--no-error-marker-scan` flag pair, or `None` to
    /// defer to the `CSA_PATTERN_INTERNAL` marker then config (#1847).
    ///
    /// Defined in this binary-only module (not on the `cli_review` struct) so
    /// `cli.rs`, which integration-test crates `#[path]`-include, stays free of
    /// `crate::`-rooted references they cannot resolve.
    pub(crate) fn error_marker_scan_override(&self) -> Option<bool> {
        crate::error_marker_scan::override_from_flags(
            self.error_marker_scan,
            self.no_error_marker_scan,
        )
    }

    pub(crate) fn resource_overrides(&self) -> RunResourceOverrides {
        RunResourceOverrides::new(self.memory_max_mb, self.min_free_memory_mb)
    }
}

pub(crate) struct DebateExecutionRequest<'a> {
    pub(crate) args: &'a DebateArgs,
    pub(crate) output_format: OutputFormat,
    pub(crate) project_root: &'a Path,
    pub(crate) config: Option<&'a ProjectConfig>,
    pub(crate) global_config: &'a GlobalConfig,
    pub(crate) model_catalog: &'a csa_config::EffectiveModelCatalog,
    pub(crate) pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    pub(crate) prompt: &'a str,
    pub(crate) debate_description: &'a str,
    pub(crate) tool: ToolName,
    pub(crate) debate_mode: DebateMode,
    pub(crate) resolved_model_spec: Option<&'a str>,
    pub(crate) resolved_tier_name: Option<&'a str>,
    pub(crate) tier_active: bool,
    pub(crate) tier_preference_order: &'a [String],
    pub(crate) debate_model: Option<&'a str>,
    pub(crate) thinking: Option<&'a str>,
    pub(crate) stream_mode: csa_process::StreamMode,
    pub(crate) timeout_seconds: Option<u64>,
    pub(crate) idle_timeout_seconds: u64,
    pub(crate) initial_response_timeout_seconds: Option<u64>,
    pub(crate) readonly_project_root: bool,
    pub(crate) startup_env: &'a StartupSubtreeEnv,
}

pub(crate) async fn execute_debate(request: DebateExecutionRequest<'_>) -> Result<i32> {
    let wall_clock_start = Instant::now();
    let candidates = tier_model_fallback::ordered_tier_candidates_with_catalog(
        request.tool,
        request.resolved_model_spec,
        request.resolved_tier_name,
        request.config,
        Some(request.global_config),
        request.model_catalog,
        tier_model_fallback::TierFallbackOptions {
            enabled: request.tier_active,
            preference_order: request.tier_preference_order,
        },
    )?;
    let effective_fast_mode = request.args.fast_but_more_cost
        || request
            .config
            .and_then(|c| c.tools.get("codex"))
            .and_then(|t| t.fast_mode)
            .unwrap_or(false)
        || request
            .global_config
            .tools
            .get("codex")
            .and_then(|t| t.fast_mode)
            .unwrap_or(false);
    warn_if_fast_mode_has_no_codex_debate_candidate(effective_fast_mode, &candidates);

    if request.args.dry_run {
        return execute_debate_dry_run(&request, &candidates, effective_fast_mode).await;
    }

    let mut execution = None;
    let mut failures = Vec::new();
    let mut final_tool = None;
    let mut final_model_spec: Option<String> = None;
    let mut failed_attempt_session = request.args.session.clone();

    'tier_attempts: for (attempt_index, (attempt_tool, attempt_model_spec)) in
        candidates.iter().enumerate()
    {
        let mut executor = crate::pipeline::build_and_validate_executor(
            attempt_tool,
            attempt_model_spec.as_deref(),
            request.debate_model,
            request.thinking,
            crate::pipeline::ConfigRefs {
                project: request.config,
                global: Some(request.global_config),
                model_catalog: Some(request.model_catalog),
            },
            request.tier_active && attempt_model_spec.is_some(),
            request.args.force_override_user_config,
            false,
        )
        .await?;
        if effective_fast_mode {
            executor.enable_codex_fast_mode();
        }
        let base_env_owned = request.global_config.build_execution_env(
            executor.tool_name(),
            debate_execution_env_options(request.args.no_failover),
        );
        // #1741: keep a pinned subtree pinned through the debater child so a
        // nested Layer-N+1 call does not re-select the tier default. Mirrors
        // csa run (run_cmd_attempt.rs). The pin is carried out-of-band as a
        // typed value (self-gated on force_ignore_tier_setting + a non-empty
        // spec) and applied by the executor's trusted channel — never via the
        // env map, so no request/config env can spoof it.
        let subtree_pin = crate::run_cmd_model_pin::resolve_subtree_model_pin(
            attempt_model_spec.as_deref(),
            request.args.force_ignore_tier_setting,
            request.args.no_failover,
        );
        let extra_env_owned = with_readonly_session_env(base_env_owned.as_ref(), true);
        let extra_env = extra_env_owned.as_ref();
        let _slot_guard = crate::pipeline::acquire_slot(&executor, request.global_config)?;
        let mut retry_count = 0u8;
        let mut first_error_context: Option<String> = None;
        let session_plan = crate::pipeline::model_failover_session::resolve_model_attempt_session(
            attempt_index,
            request.args.session.as_deref(),
            failed_attempt_session.as_deref(),
            crate::pipeline::SessionCreationMode::DaemonManaged,
            request.startup_env.session_id(),
        );
        let mut resume_session = session_plan.session_arg;
        let attempt_parent = session_plan.parent;
        let session_creation_mode = session_plan.creation_mode;

        loop {
            ensure_debate_wall_clock_within_timeout(wall_clock_start, request.timeout_seconds)?;

            let attempt_started_at = Instant::now();
            let execute_future = crate::pipeline::execute_with_session_and_meta_with_parent_source(
                &executor,
                attempt_tool,
                request.prompt,
                request.output_format,
                resume_session.clone(),
                false,
                Some(request.debate_description.to_string()),
                attempt_parent.clone(),
                request.project_root,
                request.config,
                extra_env,
                subtree_pin.as_ref(),
                false,
                Some("debate"),
                request.resolved_tier_name,
                None,
                request.stream_mode,
                request.idle_timeout_seconds,
                request.initial_response_timeout_seconds,
                None,
                None,
                Some(request.global_config),
                request.pre_session_hook.clone(),
                crate::pipeline::ParentSessionSource::ExplicitOrEnv,
                session_creation_mode,
                request.args.resource_overrides(),
                request.args.no_fs_sandbox,
                false,
                request.readonly_project_root,
                &request.args.extra_writable,
                &request.args.extra_readable,
                request.args.error_marker_scan_override(),
                false, // cli_no_hook_bypass_scan: debate has no CLI flag; defer to config
                request.startup_env,
            );

            let execute_result = if let Some(timeout_secs) = request.timeout_seconds {
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
                    if let Some(session_id) =
                        crate::pipeline::model_failover_session::extract_meta_session_id_from_error(
                            &err,
                        )
                    {
                        resume_session = Some(session_id);
                    }
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
                        failed_attempt_session = resume_session.clone();
                        continue 'tier_attempts;
                    }

                    let session_dir = resume_session.as_deref().and_then(|session_id| {
                        csa_session::get_session_dir(request.project_root, session_id).ok()
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
                                request.args.no_failover,
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
                failed_attempt_session = execution
                    .as_ref()
                    .map(|attempt| attempt.meta_session_id.clone());
                continue 'tier_attempts;
            }

            let session_dir =
                csa_session::get_session_dir(request.project_root, &executed.meta_session_id)?;
            let session_state =
                csa_session::load_session(request.project_root, &executed.meta_session_id).ok();
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
                        request.args.no_failover,
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
    let finalized = finalize_debate_outcome_with_catalog(
        request.project_root,
        request.output_format,
        execution,
        request.model_catalog,
        DebateFinalizeContext {
            all_tier_models_failed,
            project_config: request.config,

            resolved_tier_name: request.resolved_tier_name,
            failures: &failures,
            debate_mode: request.debate_mode,
            output_header: Some(DebateOutputHeader {
                prompt_bytes: request.prompt.len(),
            }),
            original_tool: Some(request.tool),
            fallback_tool: final_tool,
            fallback_reason,
            selected_model_spec: final_model_spec.as_deref(),
            tier_preference_order: request.tier_preference_order,
            fail_on_revise: request.args.fail_on_revise,
            fail_on_reject: request.args.fail_on_reject,
        },
    )?;
    print_rendered_output(finalized.rendered_output);
    Ok(finalized.exit_code)
}

async fn execute_debate_dry_run(
    request: &DebateExecutionRequest<'_>,
    candidates: &[(ToolName, Option<String>)],
    effective_fast_mode: bool,
) -> Result<i32> {
    let Some((attempt_tool, attempt_model_spec)) = candidates.first() else {
        anyhow::bail!("Debate dry-run failed: no debate tier candidates were resolved");
    };
    let mut executor = crate::pipeline::build_and_validate_executor(
        attempt_tool,
        attempt_model_spec.as_deref(),
        request.debate_model,
        request.thinking,
        crate::pipeline::ConfigRefs {
            project: request.config,
            global: Some(request.global_config),
            model_catalog: Some(request.model_catalog),
        },
        request.tier_active && attempt_model_spec.is_some(),
        request.args.force_override_user_config,
        false,
    )
    .await?;
    if effective_fast_mode {
        executor.enable_codex_fast_mode();
    }
    let summary = DebateDryRunSummary {
        session_id: create_debate_dry_run_session(
            request.project_root,
            request.debate_description,
            executor.tool_name(),
            request.resolved_tier_name,
            request.startup_env.session_id(),
        )?,
        tool: executor.tool_name().to_string(),
        model: attempt_model_spec
            .clone()
            .or_else(|| request.debate_model.map(str::to_string))
            .unwrap_or_else(|| "tool default".to_string()),
        prompt_bytes: request.prompt.len(),
        rounds: request.args.rounds,
        mode: request.debate_mode,
    };
    let rendered = render_debate_dry_run_summary(request.output_format, &summary)?;
    print_rendered_output(rendered);
    Ok(0)
}

fn print_rendered_output(rendered_output: String) {
    if rendered_output.ends_with('\n') {
        print!("{rendered_output}");
    } else {
        println!("{rendered_output}");
    }
}
