use crate::cli::ReviewArgs;
use crate::pipeline::resolve_effective_initial_response_timeout_for_tool;
#[cfg(test)]
use crate::pipeline::resolve_initial_response_timeout_for_tool;
use crate::review_consensus::CLEAN;
#[cfg(test)]
use crate::review_consensus::{consensus_strategy_label, parse_consensus_strategy};
#[cfg(test)]
use crate::review_context::discover_review_context_for_branch;
use crate::review_context::resolve_review_context;
#[cfg(test)]
use crate::review_context::{ResolvedReviewContext, ResolvedReviewContextKind};
#[cfg(test)]
use crate::review_routing::ReviewRoutingMetadata;
use crate::startup_env::StartupSubtreeEnv;
use anyhow::Result;
#[cfg(test)]
use csa_config::GlobalConfig;
#[cfg(test)]
use csa_config::ProjectConfig;
use csa_core::types::ReviewDecision;
#[cfg(test)]
use csa_core::types::ToolName;
use csa_session::state::ReviewSessionMeta;
use tracing::{debug, error, warn};
#[path = "review_cmd_output.rs"]
mod output;
use output::{is_worktree_submodule, persist_review_result_exit_code};
#[path = "review_cmd_artifact_parse.rs"]
mod artifact_parse;
#[path = "review_cmd_bug_class.rs"]
mod bug_class_pipeline;
#[path = "review_cmd_check_verdict.rs"]
mod check_verdict;
#[path = "review_cmd_diff_size.rs"]
mod diff_size;
#[path = "review_cmd_dirty_tree.rs"]
mod dirty_tree;
#[path = "review_cmd_execute.rs"]
mod execute;
#[path = "review_cmd_findings_toml.rs"]
mod findings_toml;
#[path = "review_cmd_fix.rs"]
mod fix;
#[path = "review_cmd_flow.rs"]
mod flow;
#[path = "review_cmd_gate.rs"]
mod gate;
#[path = "review_cmd_mempal.rs"]
mod mempal;
#[path = "review_cmd_multi.rs"]
mod multi;
#[path = "review_cmd_parent_artifacts.rs"]
mod parent_artifacts;
#[path = "review_cmd_post_review.rs"]
mod post_review;
#[path = "review_cmd_prior_rounds.rs"]
mod prior_rounds;
#[path = "review_cmd_prose_findings.rs"]
mod prose_findings;
#[path = "review_cmd_resolve.rs"]
mod resolve;
#[path = "review_cmd_result.rs"]
mod result_handling;
#[path = "review_cmd_reviewers.rs"]
mod reviewers;
#[path = "review_cmd_subtree_pin.rs"]
mod subtree_pin;
#[cfg(test)]
pub(crate) use bug_class_pipeline::try_extract_recurring_bug_class_skills;
#[cfg(test)]
use bug_class_pipeline::try_resolve_review_iterations;
use bug_class_pipeline::{maybe_extract_recurring_bug_class_skills, resolve_review_iterations};
#[cfg(test)]
pub(crate) use check_verdict::check_review_verdict_for_target;
#[cfg(test)]
use execute::execute_review;
use execute::{compute_diff_fingerprint, execute_review_with_tier_filter};
#[cfg(test)]
#[rustfmt::skip]
pub(crate) use flow::{ execute_review_for_tests, persist_review_sidecars_if_session_exists, should_run_fix_loop };
#[cfg(test)]
use flow::persist_review_sidecars_if_session_exists_with_diff_size;
#[cfg(not(test))]
use flow::{persist_review_sidecars_if_session_exists_with_diff_size, should_run_fix_loop};
use post_review::{build_post_review_output, emit_post_review_output, review_scope_is_cumulative};
#[rustfmt::skip]
use prior_rounds::{ explicit_review_tool, load_prior_rounds_section_or_persist_error, review_pre_exec_session_id };
#[cfg(test)]
use resolve::build_review_instruction;
#[cfg(test)]
pub(crate) use resolve::resolve_review_tool;
use resolve::{
    ReviewProjectPromptOptions, build_review_instruction_for_project, derive_scope_for_project,
    resolve_review_effective_tier, resolve_review_model, resolve_review_selection,
    resolve_review_stream_mode, resolve_review_thinking, resolve_review_tier_name,
    review_scope_allows_auto_discovery, validate_review_direct_tool_tier_restriction,
    verify_review_skill_available,
};
use result_handling::resolve_single_review_result;
#[rustfmt::skip]
use reviewers::resolve_effective_reviewer_selection_for_args;
#[cfg(test)]
#[rustfmt::skip]
pub(crate) use { fix::persist_fix_final_artifacts_for_tests, output::persist_review_verdict_for_tests };

pub(crate) async fn handle_review(
    mut args: ReviewArgs,
    current_depth: u32,
    startup_env: &StartupSubtreeEnv,
) -> Result<i32> {
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;
    if args.check_verdict {
        return check_verdict::handle_check_verdict(&project_root, &args);
    }
    let project_root_for_hooks = project_root.display().to_string();
    let Some((config, global_config)) =
        crate::pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };
    let inherited_model_pin =
        crate::run_cmd_model_pin::inherited_model_pin_from_startup(startup_env);
    let inherited_trusted_pin = subtree_pin::apply_subtree_pin(&mut args, inherited_model_pin);
    let (effective_tier, args_tool) = resolve_review_effective_tier(&args, config.as_ref())?;
    crate::run_helpers::enforce_tier_bypass_gate(crate::run_helpers::TierBypassGateCtx {
        project_config: config.as_ref(),
        global_config: &global_config,
        flags: crate::run_helpers::TierBypassGateFlags {
            model_spec: args.model_spec.is_some(),
            force: false,
            force_ignore_tier_setting: args.force_ignore_tier_setting,
            model: args.model.is_some(),
            thinking: args.thinking.is_some(),
        },
        inherited_trusted_pin,
    })?;
    validate_review_direct_tool_tier_restriction(
        args_tool.is_some(),
        config.as_ref(),
        effective_tier.as_deref(),
        args.force_override_user_config,
        args.force_ignore_tier_setting,
        args.model_spec.is_some(),
    )?;
    let pre_session_hook = csa_hooks::load_global_pre_session_hook_invocation();
    let review_pattern = verify_review_skill_available(&project_root, args.allow_fallback)?;
    if is_worktree_submodule(&project_root) {
        warn!(project_root = %project_root.display(),
            "Review inside git worktree submodule — may produce empty/unreliable output (issue #487)");
    }
    let review_gate_env = crate::build_jobs_env::build_jobs_env(args.build_jobs);
    let gate_summary = gate::run_pre_review_quality_gate(
        &project_root,
        config.as_ref(),
        &global_config,
        current_depth,
        review_gate_env.as_ref(),
    )
    .await?;

    let scope = derive_scope_for_project(&args, &project_root);
    let diff = diff_size::compute_review_diff_size(&project_root, &scope);
    let large_warn = diff_size::warn_if_large_diff(diff.as_ref(), config.as_ref(), &global_config);
    let mode = if args.fix {
        "review-and-fix"
    } else {
        "review-only"
    };
    let review_mode = args.effective_review_mode();
    let security_mode = args.effective_security_mode();
    let auto_discover_context = review_scope_allows_auto_discovery(&args);
    let prompt_file_path = args.prompt_file.as_ref().map(|p| p.display().to_string());
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

    let (mut prompt, review_routing) = build_review_instruction_for_project(
        &scope,
        mode,
        security_mode,
        review_mode,
        context.as_ref(),
        &project_root,
        ReviewProjectPromptOptions {
            project_config: config.as_ref(),
            resolved_pattern: review_pattern.as_ref(),
            prior_rounds_section: prior_rounds_section.as_deref(),
            current_session_id: startup_env.session_id(),
            full_consistency: args.full_consistency,
        },
    );

    if let Some(ref summary) = gate_summary {
        prompt.push_str("\n\n");
        prompt.push_str(summary);
    }

    diff_size::append_cross_dimension_anchor(&mut prompt, diff.as_ref(), large_warn);

    let detected_parent_tool = crate::run_helpers::detect_parent_tool();
    let parent_tool = crate::run_helpers::resolve_tool(detected_parent_tool, &global_config);
    crate::run_helpers::warn_if_tier_without_tool(args.tier.as_deref(), args_tool.is_some());
    let resolved_selection = match resolve_review_selection(
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
    let tier_preference_order = resolved_selection.tier_preference_order.clone();
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

    let stream_mode = resolve_review_stream_mode(args.stream_stdout, args.no_stream_stdout);
    let idle_timeout_seconds = crate::pipeline::resolve_effective_idle_timeout_seconds(
        config.as_ref(),
        args.idle_timeout,
        args.timeout,
    );
    let initial_response_timeout_seconds = resolve_effective_initial_response_timeout_for_tool(
        config.as_ref(),
        args.initial_response_timeout,
        args.idle_timeout,
        args.timeout,
        tool.as_str(),
    );

    let readonly_project_root =
        resolve_review_readonly_project_root(args.fix, global_config.review.readonly_sandbox);

    let reviewer_selection = resolve_effective_reviewer_selection_for_args(
        &args,
        large_warn.is_some(),
        tool,
        resolved_tier_name.as_deref(),
        config.as_ref(),
        &global_config,
    );
    let reviewers = reviewer_selection.reviewers;

    if reviewers == 1 {
        let review_future = execute_review_with_tier_filter(
            tool,
            prompt.clone(),
            args.session.clone(),
            review_model.clone(),
            resolved_model_spec.clone(),
            resolved_tier_name.clone(),
            tier_active,
            tier_preference_order.clone(),
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
            args.build_jobs,
            args.fast_but_more_cost,
            true,
            args.no_fs_sandbox,
            readonly_project_root,
            &args.extra_writable,
            &args.extra_readable,
            args.error_marker_scan_override(),
            current_depth,
            startup_env,
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

        let fallback_chain = result
            .persistable_session_id
            .as_deref()
            .and_then(|session_id| {
                csa_session::load_result(&project_root, session_id)
                    .ok()
                    .flatten()
            })
            .and_then(|saved| saved.fallback_chain)
            .unwrap_or_default();
        if let Some(diversity_warning) = crate::failover_trace::writer_family_diversity_warning(
            parent_tool.as_deref(),
            result.executed_tool,
            &fallback_chain,
        ) {
            warn!(warning = %diversity_warning, "Review heterogeneity diversity guard (#1714)");
            eprintln!("warning: {diversity_warning}");
            if let Some(session_id) = result.persistable_session_id.as_deref() {
                crate::tier_model_fallback::persist_result_warning(
                    &project_root,
                    session_id,
                    &diversity_warning,
                );
            }
        }

        let resolved =
            resolve_single_review_result(&result, result.executed_tool, &scope, &project_root);
        let sanitized = resolved.sanitized;
        let empty_output = resolved.empty_output;
        let verdict = resolved.verdict;
        let decision = resolved.decision;
        let auth_prompt_failure = resolved.auth_prompt_failure;
        print!(
            "{}",
            diff_size::add_review_diff_size_line(&sanitized, diff.as_ref())
        );
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

        let effective_exit_code = resolved.effective_exit_code;
        let diff_fingerprint = compute_diff_fingerprint(&project_root, &scope);
        let fix_attempted = should_run_fix_loop(args.fix, decision);
        let review_meta = ReviewSessionMeta {
            session_id: result.execution.meta_session_id.clone(),
            head_sha: csa_session::detect_git_head(&project_root).unwrap_or_default(),
            decision: decision.as_str().to_string(),
            verdict: verdict.to_string(),
            review_mode: Some(review_mode.as_str().to_string()),
            status_reason: result.status_reason.clone(),
            routed_to: result.routed_to.clone(),
            primary_failure: result.primary_failure.clone(),
            failure_reason: result.failure_reason.clone(),
            tool: result.executed_tool.to_string(),
            scope: scope.clone(),
            exit_code: effective_exit_code,
            fix_attempted,
            fix_rounds: 0,
            review_iterations,
            timestamp: chrono::Utc::now(),
            diff_fingerprint,
            fix_convergence: None,
        };
        let persisted_verdict_exit_code = persist_review_sidecars_if_session_exists_with_diff_size(
            &project_root,
            &review_meta,
            result.persistable_session_id.as_deref(),
            diff.as_ref(),
            large_warn,
        );
        let effective_exit_code = persisted_verdict_exit_code.unwrap_or(effective_exit_code);
        if let Some(session_id) = result.persistable_session_id.as_deref() {
            persist_review_result_exit_code(&project_root, session_id, effective_exit_code);
            diff_size::persist_review_diff_size_headers(&project_root, session_id, diff.as_ref());
        }
        if verdict != CLEAN {
            dirty_tree::maybe_emit_dirty_tree_hint(
                &project_root,
                result.persistable_session_id.as_deref(),
            );
        }
        mempal::maybe_capture_review_mempal(
            config.as_ref(),
            &global_config,
            &project_root,
            result.persistable_session_id.as_deref(),
            result.executed_tool.as_str(),
        );
        let is_cumulative_review = review_scope_is_cumulative(&scope);
        if !should_run_fix_loop(args.fix, decision) {
            post_review::suggest_review_failure_fix(&project_root, &review_meta, &sanitized);
            if verdict != CLEAN && !empty_output && !auth_prompt_failure && !is_cumulative_review {
                crate::review_findings::accumulate_findings(&project_root, &sanitized);
            }
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
            build_jobs: args.build_jobs,
            fast_but_more_cost: args.fast_but_more_cost,
            no_fs_sandbox: args.no_fs_sandbox,
            error_marker_scan_override: args.error_marker_scan_override(),
            extra_writable: &args.extra_writable,
            extra_readable: &args.extra_readable,
            timeout: args.timeout,
            diff_report: diff_size::ReviewDiffReport {
                diff_size: diff.as_ref(),
                large_diff_warning: large_warn,
            },
            project_root: &project_root,
            scope,
            decision: decision.as_str().to_string(),
            verdict: verdict.to_string(),
            review_mode: Some(review_mode.as_str().to_string()),
            max_rounds: args.max_rounds,
            initial_session_id: result.execution.meta_session_id.clone(),
            review_iterations,
            current_depth,
            startup_env,
        })
        .await;

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
            crate::review_findings::accumulate_findings(&project_root, &sanitized);
        }

        maybe_extract_recurring_bug_class_skills(&project_root, &review_session_ids);
        return fix_exit_code;
    }
    multi::run_multi_reviewer_review(multi::MultiReviewerReviewContext {
        args: &args,
        reviewers,
        selected_reviewer_tools: reviewer_selection.selected_tools,
        tool,
        prompt: &prompt,
        scope: &scope,
        project_root: &project_root,
        config: &config,
        global_config: &global_config,
        pre_session_hook: pre_session_hook.clone(),
        review_routing,
        diff_size: diff.as_ref(),
        large_diff_warning: large_warn,
        review_model,
        resolved_model_spec,
        resolved_tier_name,
        review_thinking,
        stream_mode,
        idle_timeout_seconds,
        readonly_project_root,
        build_jobs: args.build_jobs,
        prior_rounds_section: prior_rounds_section.as_deref(),
        current_session_id: startup_env.session_id(),
        current_depth,
        startup_env,
    })
    .await
}

fn resolve_review_readonly_project_root(fix: bool, configured: Option<bool>) -> bool {
    if fix {
        false
    } else {
        configured.unwrap_or(true)
    }
}

#[cfg(test)]
#[path = "review_cmd_tests_barrel.rs"]
pub(crate) mod tests;
