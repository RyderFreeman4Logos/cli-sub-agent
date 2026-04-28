use crate::cli::ReviewArgs;
use crate::pipeline::resolve_initial_response_timeout_for_tool;
use crate::review_consensus::{
    CLEAN, agreement_level, build_multi_reviewer_instruction, consensus_strategy_label,
    consensus_verdict, parse_consensus_strategy, resolve_consensus,
};
#[cfg(test)]
use crate::review_context::discover_review_context_for_branch;
use crate::review_context::resolve_review_context;
#[cfg(test)]
use crate::review_context::{ResolvedReviewContext, ResolvedReviewContextKind};
#[cfg(test)]
use crate::review_routing::ReviewRoutingMetadata;
use anyhow::{Context, Result};
#[cfg(test)]
use csa_config::GlobalConfig;
#[cfg(test)]
use csa_config::ProjectConfig;
use csa_core::consensus::AgentResponse;
use csa_core::types::ReviewDecision;
#[cfg(test)]
use csa_core::types::ToolName;
use csa_session::state::ReviewSessionMeta;
use tokio::task::JoinSet;
use tracing::{debug, error, warn};
#[path = "review_cmd_output.rs"]
mod output;
use output::{
    GEMINI_AUTH_PROMPT_STATUS_REASON, is_worktree_submodule, persist_review_meta,
    persist_review_verdict, print_reviewer_outcomes,
};
#[path = "review_cmd_bug_class.rs"]
mod bug_class_pipeline;
#[path = "review_cmd_execute.rs"]
mod execute;
#[path = "review_cmd_findings_toml.rs"]
mod findings_toml;
#[path = "review_cmd_fix.rs"]
mod fix;
#[path = "review_cmd_flow.rs"]
mod flow;
#[path = "review_cmd_post_review.rs"]
mod post_review;
#[path = "review_cmd_prior_rounds.rs"]
mod prior_rounds;
#[path = "review_cmd_resolve.rs"]
mod resolve;
#[path = "review_cmd_result.rs"]
mod result_handling;
#[path = "review_cmd_reviewers.rs"]
mod reviewers;
#[cfg(test)]
pub(crate) use bug_class_pipeline::try_extract_recurring_bug_class_skills;
#[cfg(test)]
use bug_class_pipeline::try_resolve_review_iterations;
use bug_class_pipeline::{maybe_extract_recurring_bug_class_skills, resolve_review_iterations};
use execute::{compute_diff_fingerprint, execute_review, execute_review_with_tier_filter};
use findings_toml::persist_review_findings_toml;
use flow::review_decision_from_verdict;
#[cfg(test)]
#[rustfmt::skip]
pub(crate) use flow::{ execute_review_for_tests, persist_review_sidecars_if_session_exists, should_run_fix_loop };
#[cfg(not(test))]
use flow::{persist_review_sidecars_if_session_exists, should_run_fix_loop};
use post_review::{build_post_review_output, emit_post_review_output, review_scope_is_cumulative};
#[rustfmt::skip]
use prior_rounds::{ explicit_review_tool, load_prior_rounds_section_or_persist_error, review_pre_exec_session_id };
#[cfg(test)]
use resolve::build_review_instruction;
#[cfg(test)]
pub(crate) use resolve::resolve_review_tool;
use resolve::{
    ReviewProjectPromptOptions, build_review_instruction_for_project, derive_scope,
    resolve_review_effective_tier, resolve_review_model, resolve_review_selection,
    resolve_review_stream_mode, resolve_review_thinking, resolve_review_tier_name,
    review_scope_allows_auto_discovery, verify_review_skill_available,
    write_multi_reviewer_consolidated_artifact,
};
use result_handling::{build_reviewer_outcome, resolve_single_review_result};
#[rustfmt::skip]
use reviewers::{ AutoReviewerRequest, resolve_effective_reviewer_count, resolve_multi_reviewer_pool };
#[cfg(test)]
#[rustfmt::skip]
pub(crate) use { fix::persist_fix_final_artifacts_for_tests, output::persist_review_verdict_for_tests };
pub(crate) async fn handle_review(args: ReviewArgs, current_depth: u32) -> Result<i32> {
    // 1. Determine project root
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;
    let project_root_for_hooks = project_root.display().to_string();
    // 2. Load config and validate recursion depth
    let Some((config, global_config)) =
        crate::pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };
    let pre_session_hook = csa_hooks::load_global_pre_session_hook_invocation();
    // 2b. Verify review skill is available (fail fast before any execution)
    verify_review_skill_available(&project_root, args.allow_fallback)?;
    // Warn if running inside a worktree submodule (known limitation, see #487)
    if is_worktree_submodule(&project_root) {
        warn!(project_root = %project_root.display(),
            "Review inside git worktree submodule — may produce empty/unreliable output (issue #487)");
    }
    // 2c. Run pre-review quality gate pipeline (after skill check, before tool execution)
    let gate_summary = {
        let gate_steps = global_config.review.effective_gate_steps();
        let gate_timeout = config
            .as_ref()
            .and_then(|c| c.review.as_ref())
            .map(|r| r.gate_timeout_secs)
            .unwrap_or_else(csa_config::ReviewConfig::default_gate_timeout);
        let gate_mode = &global_config.review.gate_mode;
        if gate_steps.is_empty() {
            // Legacy path: use single gate_command with auto-detection fallback
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
                None
            } else if !gate_result.passed() {
                match gate_mode {
                    csa_config::GateMode::Monitor => {
                        warn!(
                            command = %gate_result.command,
                            exit_code = ?gate_result.exit_code,
                            "Quality gate failed (monitor mode — continuing with review)"
                        );
                        None
                    }
                    csa_config::GateMode::CriticalOnly | csa_config::GateMode::Full => {
                        let mut msg = format!(
                            "Pre-review quality gate failed (mode={gate_mode:?}).\n\
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
                None
            }
        } else {
            // Multi-step pipeline: L1 → L2 → L3 sequential execution
            let pipeline_result = crate::pipeline::gate::evaluate_quality_gates(
                &project_root,
                &gate_steps,
                gate_timeout,
                gate_mode,
            )
            .await?;

            let summary = pipeline_result.summary_for_review();

            if !pipeline_result.passed {
                match gate_mode {
                    csa_config::GateMode::Monitor => {
                        warn!("Quality gate pipeline failed (monitor mode — continuing)");
                        Some(summary)
                    }
                    csa_config::GateMode::CriticalOnly | csa_config::GateMode::Full => {
                        let failed = pipeline_result.failed_step.as_deref().unwrap_or("unknown");
                        // Include gate output in error for diagnostics
                        let mut msg = format!(
                            "Pre-review quality gate pipeline FAILED at step: {failed}\n\
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
            } else {
                debug!("Quality gate pipeline passed");
                Some(summary)
            }
        }
    };

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
    // --prompt-file provides a path (like --context), not inline content.
    let prompt_file_path = args.prompt_file.as_ref().map(|p| p.display().to_string());
    // --spec takes priority over --context / --prompt-file for explicit spec-based review
    let explicit_context = args
        .spec
        .as_deref()
        .or(args.context.as_deref())
        .or(prompt_file_path.as_deref());
    let context = resolve_review_context(explicit_context, &project_root, auto_discover_context)?;

    debug!(
        scope = %scope,
        mode = %mode,
        review_mode = %review_mode,
        security_mode = %security_mode,
        auto_discover_context,
        has_context = context.is_some(),
        "Review parameters"
    );
    let review_description = format!(
        "review: {}",
        crate::run_helpers::truncate_prompt(&scope, 80)
    );
    let prior_rounds_section =
        load_prior_rounds_section_or_persist_error(&args, &project_root, &review_description)?;

    // 4. Build review instruction (no diff content — tool loads skill and fetches diff itself)
    let (mut prompt, review_routing) = build_review_instruction_for_project(
        &scope,
        mode,
        security_mode,
        review_mode,
        context.as_ref(),
        &project_root,
        ReviewProjectPromptOptions {
            project_config: config.as_ref(),
            prior_rounds_section: prior_rounds_section.as_deref(),
            full_consistency: args.full_consistency,
        },
    );

    // 4b. Inject gate pipeline results into review prompt for reviewer awareness
    if let Some(ref summary) = gate_summary {
        prompt.push_str("\n\n");
        prompt.push_str(summary);
    }

    let detected_parent_tool = crate::run_helpers::detect_parent_tool();
    let parent_tool = crate::run_helpers::resolve_tool(detected_parent_tool, &global_config);
    let effective_tier = resolve_review_effective_tier(&args, config.as_ref())?;
    let resolved_selection = match resolve_review_selection(
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
                    session_id: review_pre_exec_session_id(&args),
                    description: Some(review_description.as_str()),
                    parent: None,
                    tool_name: explicit_review_tool(&args).map(|tool| tool.as_str()),
                    task_type: Some("review"),
                    tier_name: effective_tier.as_deref(),
                    error: err,
                },
            ));
        }
    };
    let tool = resolved_selection.tool;
    let resolved_model_spec = resolved_selection.model_spec.clone();
    let tier_filter = resolved_selection.tier_filter.clone();
    let tier_active = resolved_model_spec.is_some()
        && args.model_spec.is_none()
        && !args.force_ignore_tier_setting;
    let resolved_tier_name = if tier_active {
        resolve_review_tier_name(
            config.as_ref(),
            &global_config,
            effective_tier.as_deref(),
            args.force_override_user_config,
            args.force_ignore_tier_setting,
        )?
    } else {
        None
    };

    let config_review_model = config
        .as_ref()
        .and_then(|c| c.review.as_ref())
        .and_then(|r| r.model.as_deref())
        .or(global_config.review.model.as_deref());
    let review_model = resolve_review_model(
        args.model.as_deref(),
        config_review_model,
        resolved_model_spec.is_some(),
    );

    let review_thinking = resolve_review_thinking(
        args.thinking.as_deref(),
        config
            .as_ref()
            .and_then(|c| c.review.as_ref())
            .and_then(|r| r.thinking.as_deref())
            .or(global_config.review.thinking.as_deref()),
        resolved_model_spec.is_some(),
    );

    // Resolve stream mode from CLI flags (default: BufferOnly for review)
    let stream_mode = resolve_review_stream_mode(args.stream_stdout, args.no_stream_stdout);
    let idle_timeout_seconds =
        crate::pipeline::resolve_idle_timeout_seconds(config.as_ref(), args.idle_timeout);
    let initial_response_timeout_seconds = resolve_initial_response_timeout_for_tool(
        config.as_ref(),
        args.initial_response_timeout,
        args.idle_timeout,
        tool.as_str(),
    );

    // Resolve readonly_project_root from config (default: false).
    let readonly_project_root = global_config.review.readonly_sandbox.unwrap_or(false);

    let requested_reviewers = args.requested_reviewers() as usize;
    let reviewers = resolve_effective_reviewer_count(&AutoReviewerRequest {
        requested_reviewers,
        explicit_reviewer_count: args.reviewers.is_some(),
        single: args.single,
        scope_is_range: args.range.is_some(),
        explicit_tool: explicit_review_tool(&args),
        explicit_model_spec: args.model_spec.as_deref(),
        primary_tool: tool,
        resolved_tier_name: resolved_tier_name.as_deref(),
        config: config.as_ref(),
        global_config: &global_config,
    });

    if reviewers == 1 {
        // Single-reviewer path (with optional --fix loop).
        let review_future = execute_review_with_tier_filter(
            tool,
            prompt.clone(),
            args.session.clone(),
            review_model.clone(),
            resolved_model_spec.clone(),
            resolved_tier_name.clone(),
            tier_active,
            tier_filter.clone(),
            review_thinking.clone(),
            review_description.clone(),
            &project_root,
            config.as_ref(),
            &global_config,
            pre_session_hook.clone(),
            review_routing.clone(),
            stream_mode,
            idle_timeout_seconds,
            initial_response_timeout_seconds,
            args.force_override_user_config,
            args.force_ignore_tier_setting,
            args.no_failover,
            args.no_fs_sandbox,
            readonly_project_root,
            &args.extra_writable,
            &args.extra_readable,
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

        let resolved = resolve_single_review_result(&result, result.executed_tool, &scope);
        let sanitized = resolved.sanitized;
        let empty_output = resolved.empty_output;
        let verdict = resolved.verdict;
        let decision = resolved.decision;
        let auth_prompt_failure = resolved.auth_prompt_failure;
        print!("{}", sanitized);
        debug!(verdict, decision = %decision, empty_output, "Review verdict (legacy + four-value)");
        let review_iterations = result
            .persistable_session_id
            .as_deref()
            .map_or(0, |session_id| {
                resolve_review_iterations(&project_root, session_id)
            });
        let review_session_ids = result
            .persistable_session_id
            .iter()
            .cloned()
            .collect::<Vec<_>>();

        // Write structured review metadata to session directory.
        let effective_exit_code = resolved.effective_exit_code;
        let diff_fingerprint = compute_diff_fingerprint(&project_root, &scope);
        let review_meta = ReviewSessionMeta {
            session_id: result.execution.meta_session_id.clone(),
            head_sha: csa_session::detect_git_head(&project_root).unwrap_or_default(),
            decision: decision.as_str().to_string(),
            verdict: verdict.to_string(),
            status_reason: result.status_reason.clone(),
            routed_to: result.routed_to.clone(),
            primary_failure: result.primary_failure.clone(),
            failure_reason: result.failure_reason.clone(),
            tool: result.executed_tool.to_string(),
            scope: scope.clone(),
            exit_code: effective_exit_code,
            fix_attempted: args.fix,
            fix_rounds: 0,
            review_iterations,
            timestamp: chrono::Utc::now(),
            diff_fingerprint,
        };
        persist_review_sidecars_if_session_exists(
            &project_root,
            &review_meta,
            result.persistable_session_id.as_deref(),
        );

        let is_cumulative_review = review_scope_is_cumulative(&scope);

        if !should_run_fix_loop(args.fix, decision) {
            // Accumulate only on FINAL result to avoid double-counting when --fix resolves the same issues.
            if verdict != CLEAN && !empty_output && !auth_prompt_failure && !is_cumulative_review {
                crate::review_findings::accumulate_findings(&project_root, &sanitized);
            }
            // PostReview hook: only for final results (no fix loop pending).
            let post_review_output = build_post_review_output(
                &crate::pipeline::capture_observational_hook_output(
                    csa_hooks::HookEvent::PostReview,
                    &[
                        ("session_id", result.execution.meta_session_id.as_str()),
                        ("decision", decision.as_str()),
                        ("verdict", verdict),
                        ("scope", &scope),
                        ("project_root", project_root_for_hooks.as_str()),
                    ],
                    &project_root,
                ),
                decision,
                &scope,
            );
            emit_post_review_output(&post_review_output);
            maybe_extract_recurring_bug_class_skills(&project_root, &review_session_ids);
            return Ok(effective_exit_code);
        }

        // Skip --fix when the effective review tool cannot edit existing files.
        let effective_fix_tool = result.executed_tool;
        let effective_fix_model_spec = result.routed_to.clone().or_else(|| {
            (effective_fix_tool == tool)
                .then(|| resolved_model_spec.clone())
                .flatten()
        });
        let tool_can_edit = config
            .as_ref()
            .is_none_or(|cfg| cfg.can_tool_edit_existing(effective_fix_tool.as_str()));
        if !tool_can_edit {
            warn!(
                tool = %effective_fix_tool,
                "--fix requested but tool has allow_edit_existing_files=false; skipping fix loop"
            );
            maybe_extract_recurring_bug_class_skills(&project_root, &review_session_ids);
            return Ok(effective_exit_code);
        }
        // Resume the effective review session to apply fixes, then re-gate.
        let scope_for_hook = scope.clone();
        let fix_exit_code = fix::run_fix_loop(fix::FixLoopContext {
            effective_tool: effective_fix_tool,
            config: config.as_ref(),
            global_config: &global_config,
            review_model,
            effective_tier_model_spec: effective_fix_model_spec,
            review_thinking,
            review_routing,
            stream_mode,
            idle_timeout_seconds,
            initial_response_timeout_seconds,
            force_override_user_config: args.force_override_user_config,
            force_ignore_tier_setting: args.force_ignore_tier_setting,
            no_failover: args.no_failover,
            no_fs_sandbox: args.no_fs_sandbox,
            extra_writable: &args.extra_writable,
            extra_readable: &args.extra_readable,
            timeout: args.timeout,
            project_root: &project_root,
            scope,
            decision: decision.as_str().to_string(),
            verdict: verdict.to_string(),
            max_rounds: args.max_rounds,
            initial_session_id: result.execution.meta_session_id.clone(),
            review_iterations,
        })
        .await;

        // Fire PostReview hook after fix loop completes; forward stdout so callers can chain the next step mechanically.
        let fix_passed = matches!(&fix_exit_code, Ok(0));
        let post_review_output = build_post_review_output(
            &crate::pipeline::capture_observational_hook_output(
                csa_hooks::HookEvent::PostReview,
                &[
                    ("session_id", result.execution.meta_session_id.as_str()),
                    ("decision", if fix_passed { "pass" } else { "fail" }),
                    ("verdict", if fix_passed { CLEAN } else { verdict }),
                    ("scope", &scope_for_hook),
                    ("project_root", project_root_for_hooks.as_str()),
                ],
                &project_root,
            ),
            if fix_passed {
                ReviewDecision::Pass
            } else {
                ReviewDecision::Fail
            },
            &scope_for_hook,
        );
        if fix_passed {
            emit_post_review_output(&post_review_output);
        } else if !is_cumulative_review {
            // Fix exhausted — accumulate original findings for promotion.
            crate::review_findings::accumulate_findings(&project_root, &sanitized);
        }

        maybe_extract_recurring_bug_class_skills(&project_root, &review_session_ids);

        return fix_exit_code;
    }

    if args.fix {
        anyhow::bail!("--fix is not supported when --reviewers > 1");
    }
    if args.session.is_some() {
        anyhow::bail!("--session is only supported when --reviewers=1");
    }

    let consensus_strategy = parse_consensus_strategy(&args.consensus)?;
    let reviewer_pool = resolve_multi_reviewer_pool(
        reviewers,
        explicit_review_tool(&args),
        tool,
        resolved_tier_name.as_deref(),
        config.as_ref(),
        &global_config,
    )?;
    let reviewer_tools = reviewer_pool.reviewer_tools;
    let tier_reviewer_specs = reviewer_pool.tier_reviewer_specs;

    let mut join_set = JoinSet::new();
    for (reviewer_index, reviewer_tool) in reviewer_tools.into_iter().enumerate() {
        let reviewer_prompt = build_multi_reviewer_instruction(
            &prompt,
            reviewer_index + 1,
            reviewer_tool,
            &project_root,
            prior_rounds_section.as_deref(),
        );
        let reviewer_model = review_model.clone();
        let reviewer_project_root = project_root.clone();
        let reviewer_config = config.clone();
        let reviewer_global = global_config.clone();
        let reviewer_pre_session_hook = pre_session_hook.clone();
        let reviewer_description = format!(
            "review[{}]: {}",
            reviewer_index + 1,
            crate::run_helpers::truncate_prompt(&scope, 80)
        );
        let reviewer_routing = review_routing.clone();

        let reviewer_force_override = args.force_override_user_config;
        let reviewer_no_fs_sandbox = args.no_fs_sandbox;
        let reviewer_extra_writable = args.extra_writable.clone();
        let reviewer_extra_readable = args.extra_readable.clone();
        // Keep every reviewer on the resolved tier when possible by binding
        // each tool to its tier model spec. Fall back to the primary spec only
        // when we only have a single tier-resolved reviewer tool.
        let reviewer_model_spec = tier_reviewer_specs
            .iter()
            .find(|resolution| resolution.tool == reviewer_tool)
            .map(|resolution| resolution.model_spec.clone())
            .or_else(|| {
                if reviewer_tool == tool {
                    resolved_model_spec.clone()
                } else {
                    None
                }
            });
        let reviewer_tier_name = resolved_tier_name.clone();
        let reviewer_thinking = review_thinking.clone();
        let reviewer_initial_response_timeout_seconds = resolve_initial_response_timeout_for_tool(
            reviewer_config.as_ref(),
            args.initial_response_timeout,
            args.idle_timeout,
            reviewer_tool.as_str(),
        );
        join_set.spawn(async move {
            let session_result = execute_review(
                reviewer_tool,
                reviewer_prompt,
                None,
                reviewer_model,
                reviewer_model_spec,
                reviewer_tier_name,
                false,
                reviewer_thinking,
                reviewer_description,
                &reviewer_project_root,
                reviewer_config.as_ref(),
                &reviewer_global,
                reviewer_pre_session_hook,
                reviewer_routing,
                stream_mode,
                idle_timeout_seconds,
                reviewer_initial_response_timeout_seconds,
                reviewer_force_override,
                args.force_ignore_tier_setting,
                args.no_failover,
                reviewer_no_fs_sandbox,
                readonly_project_root,
                &reviewer_extra_writable,
                &reviewer_extra_readable,
            )
            .await?;
            build_reviewer_outcome(reviewer_index, reviewer_tool, &session_result)
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

    let review_iterations = outcomes
        .first()
        .map(|outcome| resolve_review_iterations(&project_root, &outcome.session_id))
        .unwrap_or(1);
    let review_meta_timestamp = chrono::Utc::now();
    let head_sha = csa_session::detect_git_head(&project_root).unwrap_or_default();
    let diff_fingerprint = compute_diff_fingerprint(&project_root, &scope);

    for outcome in &outcomes {
        let review_meta = ReviewSessionMeta {
            session_id: outcome.session_id.clone(),
            head_sha: head_sha.clone(),
            decision: review_decision_from_verdict(outcome.verdict)
                .as_str()
                .to_string(),
            verdict: outcome.verdict.to_string(),
            status_reason: (outcome.verdict == "UNCERTAIN"
                && outcome
                    .diagnostic
                    .as_deref()
                    .is_some_and(|d| d.contains("OAuth browser prompt")))
            .then(|| GEMINI_AUTH_PROMPT_STATUS_REASON.to_string()),
            routed_to: None,
            primary_failure: None,
            failure_reason: None,
            tool: outcome.tool.as_str().to_string(),
            scope: scope.clone(),
            exit_code: outcome.exit_code,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations,
            timestamp: review_meta_timestamp,
            diff_fingerprint: diff_fingerprint.clone(),
        };
        persist_review_meta(&project_root, &review_meta);
        persist_review_verdict(&project_root, &review_meta, &[], Vec::new());
        persist_review_findings_toml(&project_root, &review_meta);
    }

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

    print_reviewer_outcomes(&outcomes);

    println!(
        "===== Consensus =====\nstrategy: {}\nconsensus_reached: {}\nagreement_level: {:.0}%\nfinal_decision: {final_verdict}\nindividual_verdicts:",
        consensus_strategy_label(consensus_result.strategy_used),
        consensus_result.consensus_reached,
        agreement * 100.0,
    );
    for o in &outcomes {
        println!(
            "- reviewer {} ({}) => {}",
            o.reviewer_index + 1,
            o.tool,
            o.verdict
        );
    }

    let review_session_ids = outcomes
        .iter()
        .map(|outcome| outcome.session_id.clone())
        .collect::<Vec<_>>();
    // Do not persist inherited CSA_SESSION_ID review metadata here; unlike the
    // single-reviewer path, that would overwrite an unrelated parent session.

    maybe_extract_recurring_bug_class_skills(&project_root, &review_session_ids);

    Ok(if final_verdict == CLEAN { 0 } else { 1 })
}

#[cfg(test)]
#[path = "review_cmd_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "review_cmd_tests_full_consistency.rs"]
mod tests_full_consistency;
#[cfg(test)]
#[path = "review_cmd_timeout_tests.rs"]
mod timeout_tests;
