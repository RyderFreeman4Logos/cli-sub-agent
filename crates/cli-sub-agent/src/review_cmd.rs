use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};

use crate::cli::ReviewArgs;
use crate::review_consensus::{
    CLEAN, HAS_ISSUES, agreement_level, build_multi_reviewer_instruction, build_reviewer_tools,
    consensus_strategy_label, consensus_verdict, parse_consensus_strategy, parse_review_decision,
    parse_review_verdict, resolve_consensus,
};
#[cfg(test)]
use crate::review_context::discover_review_context_for_branch;
use crate::review_context::resolve_review_context;
#[cfg(test)]
use crate::review_context::{ResolvedReviewContext, ResolvedReviewContextKind};
use crate::review_routing::{ReviewRoutingMetadata, persist_review_routing_artifact};
use csa_config::{ExecutionEnvOptions, GlobalConfig, ProjectConfig};
use csa_core::consensus::AgentResponse;
use csa_core::types::{OutputFormat, ToolName};
use csa_session::state::ReviewSessionMeta;

#[path = "review_cmd_output.rs"]
mod output;
use output::{
    ReviewerOutcome, is_review_output_empty, is_worktree_submodule, persist_review_meta,
    sanitize_review_output,
};

#[path = "review_cmd_resolve.rs"]
mod resolve;
#[cfg(test)]
use resolve::build_review_instruction;
use resolve::{
    ANTI_RECURSION_PREAMBLE, build_review_instruction_for_project, derive_scope,
    resolve_review_stream_mode, resolve_review_thinking, resolve_review_tool,
    review_scope_allows_auto_discovery, verify_review_skill_available,
    write_multi_reviewer_consolidated_artifact,
};

pub(crate) async fn handle_review(args: ReviewArgs, current_depth: u32) -> Result<i32> {
    // 1. Determine project root
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;

    // 2. Load config and validate recursion depth
    let Some((config, global_config)) =
        crate::pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };

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

    // 4. Build review instruction (no diff content — tool loads skill and fetches diff itself)
    let (mut prompt, review_routing) = build_review_instruction_for_project(
        &scope,
        mode,
        security_mode,
        review_mode,
        context.as_ref(),
        &project_root,
        config.as_ref(),
    );

    // 4b. Inject gate pipeline results into review prompt for reviewer awareness
    if let Some(ref summary) = gate_summary {
        prompt.push_str("\n\n");
        prompt.push_str(summary);
    }

    // 5. Determine tool (with tier-based resolution)
    let detected_parent_tool = crate::run_helpers::detect_parent_tool();
    let parent_tool = crate::run_helpers::resolve_tool(detected_parent_tool, &global_config);
    let (tool, tier_model_spec) = resolve_review_tool(
        args.tool,
        config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
        &project_root,
        args.force_override_user_config,
        args.tier.as_deref(),
        args.force_ignore_tier_setting,
    )?;

    // Resolve model: CLI --model > project config review.model > global config review.model.
    // When tier is also set, build_executor applies model override after tier spec construction.
    let review_model = args.model.clone().or_else(|| {
        config
            .as_ref()
            .and_then(|c| c.review.as_ref())
            .and_then(|r| r.model.clone())
            .or_else(|| global_config.review.model.clone())
    });

    // Resolve thinking: CLI > config review.thinking > tier model_spec thinking.
    // Tier thinking is embedded in model_spec and applied via build_and_validate_executor.
    let review_thinking = resolve_review_thinking(
        args.thinking.as_deref(),
        config
            .as_ref()
            .and_then(|c| c.review.as_ref())
            .and_then(|r| r.thinking.as_deref())
            .or(global_config.review.thinking.as_deref()),
    );

    // Resolve stream mode from CLI flags (default: BufferOnly for review)
    let stream_mode = resolve_review_stream_mode(args.stream_stdout, args.no_stream_stdout);
    let idle_timeout_seconds =
        crate::pipeline::resolve_idle_timeout_seconds(config.as_ref(), args.idle_timeout);
    let initial_response_timeout_seconds =
        crate::pipeline::resolve_initial_response_timeout_seconds(
            config.as_ref(),
            args.initial_response_timeout,
        );

    // Resolve readonly_project_root from config (default: false).
    let readonly_project_root = global_config.review.readonly_sandbox.unwrap_or(false);

    if args.reviewers == 1 {
        // Single-reviewer path (with optional --fix loop).
        let review_future = execute_review(
            tool,
            prompt.clone(),
            args.session.clone(),
            review_model.clone(),
            tier_model_spec.clone(),
            review_thinking.clone(),
            format!(
                "review: {}",
                crate::run_helpers::truncate_prompt(&scope, 80)
            ),
            &project_root,
            config.as_ref(),
            &global_config,
            review_routing.clone(),
            stream_mode,
            idle_timeout_seconds,
            initial_response_timeout_seconds,
            args.force_override_user_config,
            args.no_fs_sandbox,
            readonly_project_root,
            &args.extra_writable,
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

        let sanitized = sanitize_review_output(&result.execution.output);
        let empty_output = is_review_output_empty(&result.execution.output);
        if empty_output {
            warn!(scope = %scope, tool = %tool, session_id = %result.meta_session_id,
                "Review produced no substantive output — tool may have failed silently. \
                 Check: csa session logs {}", result.meta_session_id);
        }
        print!("{}", sanitized);

        // Empty output forces non-clean verdict to prevent silent pass-through.
        let verdict = if empty_output {
            HAS_ISSUES
        } else {
            parse_review_verdict(&result.execution.output, result.execution.exit_code)
        };
        let decision = if empty_output {
            csa_core::types::ReviewDecision::Uncertain
        } else {
            parse_review_decision(&result.execution.output, result.execution.exit_code)
        };
        debug!(verdict, decision = %decision, empty_output, "Review verdict (legacy + four-value)");

        // Write structured review metadata to session directory.
        let effective_exit_code = if empty_output {
            1
        } else {
            result.execution.exit_code
        };
        persist_review_meta(
            &project_root,
            &ReviewSessionMeta {
                session_id: result.meta_session_id.clone(),
                head_sha: csa_session::detect_git_head(&project_root).unwrap_or_default(),
                decision: decision.as_str().to_string(),
                verdict: verdict.to_string(),
                tool: tool.to_string(),
                scope: scope.clone(),
                exit_code: effective_exit_code,
                fix_attempted: args.fix,
                fix_rounds: 0,
                timestamp: chrono::Utc::now(),
            },
        );

        if !args.fix || verdict == CLEAN {
            return Ok(effective_exit_code);
        }

        // --- Fix loop: resume the review session to apply fixes, then re-gate ---
        let max_rounds = args.max_rounds;
        let mut session_id = result.meta_session_id.clone();

        for round in 1..=max_rounds {
            info!(round, max_rounds, session_id = %session_id, "Fix round starting");

            // Step 1: Resume the review session with a fix prompt
            let fix_prompt = format!(
                "{ANTI_RECURSION_PREAMBLE}\
                 Fix round {round}/{max_rounds}.\n\
                 Fix all issues found in the review. Run formatting and linting commands as needed.\n\
                 After applying fixes, verify the changes compile and pass basic checks.\n\
                 If no issues remain, emit verdict: CLEAN."
            );

            let fix_future = execute_review(
                tool,
                fix_prompt,
                Some(session_id.clone()),
                review_model.clone(),
                tier_model_spec.clone(),
                review_thinking.clone(),
                format!("fix round {round}/{max_rounds}"),
                &project_root,
                config.as_ref(),
                &global_config,
                review_routing.clone(),
                stream_mode,
                idle_timeout_seconds,
                initial_response_timeout_seconds,
                args.force_override_user_config,
                args.no_fs_sandbox,
                false, // fix pass must write — override readonly_project_root
                &args.extra_writable,
            );

            let fix_result = if let Some(timeout_secs) = args.timeout {
                match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fix_future)
                    .await
                {
                    Ok(inner) => inner?,
                    Err(_) => {
                        error!(
                            timeout_secs = timeout_secs,
                            round, "Fix round aborted: wall-clock timeout exceeded"
                        );
                        anyhow::bail!(
                            "Fix round {round}/{max_rounds} aborted: --timeout {timeout_secs}s exceeded."
                        );
                    }
                }
            } else {
                fix_future.await?
            };

            print!("{}", sanitize_review_output(&fix_result.execution.output));
            let fix_empty = is_review_output_empty(&fix_result.execution.output);
            if fix_empty {
                warn!(
                    round,
                    "Fix round produced no substantive output — treating as failed"
                );
            }
            session_id = fix_result.meta_session_id.clone();

            // Step 2: Run the quality gate after fix
            let fix_gate_steps = global_config.review.effective_gate_steps();
            let fix_gate_timeout = config
                .as_ref()
                .and_then(|c| c.review.as_ref())
                .map(|r| r.gate_timeout_secs)
                .unwrap_or_else(csa_config::ReviewConfig::default_gate_timeout);
            let fix_gate_mode = &global_config.review.gate_mode;

            let gate_passed = if fix_gate_steps.is_empty() {
                let gate_command = config
                    .as_ref()
                    .and_then(|c| c.review.as_ref())
                    .and_then(|r| r.gate_command.as_deref());
                let gate_result = crate::pipeline::gate::evaluate_quality_gate(
                    &project_root,
                    gate_command,
                    fix_gate_timeout,
                    fix_gate_mode,
                )
                .await?;

                if !gate_result.passed() {
                    warn!(
                        round,
                        max_rounds,
                        command = %gate_result.command,
                        exit_code = ?gate_result.exit_code,
                        "Quality gate still failing after fix round"
                    );
                }
                gate_result.passed()
            } else {
                let pipeline_result = crate::pipeline::gate::evaluate_quality_gates(
                    &project_root,
                    &fix_gate_steps,
                    fix_gate_timeout,
                    fix_gate_mode,
                )
                .await?;

                if !pipeline_result.passed {
                    warn!(
                        round,
                        max_rounds,
                        failed_step = ?pipeline_result.failed_step,
                        "Quality gate pipeline still failing after fix round"
                    );
                }
                pipeline_result.passed
            };

            if gate_passed && !fix_empty {
                info!(round, "Fix round succeeded — quality gate passed");
                // Update meta: fix succeeded
                persist_review_meta(
                    &project_root,
                    &ReviewSessionMeta {
                        session_id: session_id.clone(),
                        head_sha: csa_session::detect_git_head(&project_root).unwrap_or_default(),
                        decision: "pass".to_string(),
                        verdict: CLEAN.to_string(),
                        tool: tool.to_string(),
                        scope: scope.clone(),
                        exit_code: 0,
                        fix_attempted: true,
                        fix_rounds: u32::from(round),
                        timestamp: chrono::Utc::now(),
                    },
                );
                return Ok(0);
            }
        }

        // All fix rounds exhausted; gate still fails.
        // Update meta: fix exhausted
        persist_review_meta(
            &project_root,
            &ReviewSessionMeta {
                session_id: result.meta_session_id.clone(),
                head_sha: csa_session::detect_git_head(&project_root).unwrap_or_default(),
                decision: decision.as_str().to_string(),
                verdict: verdict.to_string(),
                tool: tool.to_string(),
                scope: scope.clone(),
                exit_code: 1,
                fix_attempted: true,
                fix_rounds: u32::from(max_rounds),
                timestamp: chrono::Utc::now(),
            },
        );
        error!(
            max_rounds,
            "All fix rounds exhausted — quality gate still failing"
        );
        return Ok(1);
    }

    if args.fix {
        anyhow::bail!("--fix is not supported when --reviewers > 1");
    }
    if args.session.is_some() {
        anyhow::bail!("--session is only supported when --reviewers=1");
    }

    let reviewers = args.reviewers as usize;
    let consensus_strategy = parse_consensus_strategy(&args.consensus)?;
    let reviewer_tools = build_reviewer_tools(
        args.tool,
        tool,
        config.as_ref(),
        Some(&global_config),
        reviewers,
    );

    let mut join_set = JoinSet::new();
    for (reviewer_index, reviewer_tool) in reviewer_tools.into_iter().enumerate() {
        let reviewer_prompt =
            build_multi_reviewer_instruction(&prompt, reviewer_index + 1, reviewer_tool);
        let reviewer_model = review_model.clone();
        let reviewer_project_root = project_root.clone();
        let reviewer_config = config.clone();
        let reviewer_global = global_config.clone();
        let reviewer_description = format!(
            "review[{}]: {}",
            reviewer_index + 1,
            crate::run_helpers::truncate_prompt(&scope, 80)
        );
        let reviewer_routing = review_routing.clone();

        let reviewer_force_override = args.force_override_user_config;
        let reviewer_no_fs_sandbox = args.no_fs_sandbox;
        let reviewer_extra_writable = args.extra_writable.clone();
        // Only pass tier_model_spec to the reviewer whose tool matches the
        // tier-resolved primary tool.  For other reviewers (selected for
        // heterogeneity), the model_spec would override their tool via
        // Executor::from_spec, collapsing cognitive diversity.
        let reviewer_model_spec = if reviewer_tool == tool {
            tier_model_spec.clone()
        } else {
            None
        };
        let reviewer_thinking = review_thinking.clone();
        join_set.spawn(async move {
            let session_result = execute_review(
                reviewer_tool,
                reviewer_prompt,
                None,
                reviewer_model,
                reviewer_model_spec,
                reviewer_thinking,
                reviewer_description,
                &reviewer_project_root,
                reviewer_config.as_ref(),
                &reviewer_global,
                reviewer_routing,
                stream_mode,
                idle_timeout_seconds,
                initial_response_timeout_seconds,
                reviewer_force_override,
                reviewer_no_fs_sandbox,
                readonly_project_root,
                &reviewer_extra_writable,
            )
            .await?;
            let result = &session_result.execution;
            let empty = is_review_output_empty(&result.output);
            if empty {
                tracing::warn!(
                    reviewer = reviewer_index + 1,
                    tool = %reviewer_tool,
                    "Reviewer produced no substantive output — may have failed silently"
                );
            }
            Ok::<ReviewerOutcome, anyhow::Error>(ReviewerOutcome {
                reviewer_index,
                tool: reviewer_tool,
                verdict: if empty {
                    HAS_ISSUES
                } else {
                    parse_review_verdict(&result.output, result.exit_code)
                },
                output: sanitize_review_output(&result.output),
                exit_code: if empty { 1 } else { result.exit_code },
            })
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

    for outcome in &outcomes {
        println!(
            "===== Reviewer {} ({}) | verdict={} | exit_code={} =====",
            outcome.reviewer_index + 1,
            outcome.tool,
            outcome.verdict,
            outcome.exit_code
        );
        print!("{}", outcome.output);
        if !outcome.output.ends_with('\n') {
            println!();
        }
    }

    println!("===== Consensus =====");
    println!(
        "strategy: {}",
        consensus_strategy_label(consensus_result.strategy_used)
    );
    println!("consensus_reached: {}", consensus_result.consensus_reached);
    println!("agreement_level: {:.0}%", agreement * 100.0);
    println!("final_decision: {final_verdict}");
    println!("individual_verdicts:");
    for outcome in &outcomes {
        println!(
            "- reviewer {} ({}) => {}",
            outcome.reviewer_index + 1,
            outcome.tool,
            outcome.verdict
        );
    }

    Ok(if final_verdict == CLEAN { 0 } else { 1 })
}

#[allow(clippy::too_many_arguments)]
async fn execute_review(
    tool: ToolName,
    prompt: String,
    session: Option<String>,
    model: Option<String>,
    tier_model_spec: Option<String>,
    thinking: Option<String>,
    description: String,
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    review_routing: ReviewRoutingMetadata,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    force_override_user_config: bool,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
) -> Result<crate::pipeline::SessionExecutionResult> {
    let enforce_tier = tier_model_spec.is_some();
    let executor = crate::pipeline::build_and_validate_executor(
        &tool,
        tier_model_spec.as_deref(),
        model.as_deref(),
        thinking.as_deref(),
        crate::pipeline::ConfigRefs {
            project: project_config,
            global: Some(global_config),
        },
        enforce_tier,
        force_override_user_config,
        false, // review must not inherit `csa run` per-tool defaults
    )
    .await?;

    let can_edit =
        project_config.is_none_or(|cfg| cfg.can_tool_edit_existing(executor.tool_name()));
    let can_write_new =
        project_config.is_none_or(|cfg| cfg.can_tool_write_new(executor.tool_name()));
    let effective_prompt = if !can_edit || !can_write_new {
        info!(
            tool = %executor.tool_name(),
            can_edit,
            can_write_new,
            "Applying filesystem restrictions via prompt injection"
        );
        executor.apply_restrictions(&prompt, can_edit, can_write_new)
    } else {
        prompt
    };

    let extra_env_owned = global_config.build_execution_env(
        executor.tool_name(),
        ExecutionEnvOptions::with_no_flash_fallback(),
    );
    let extra_env = extra_env_owned.as_ref();
    let _slot_guard = crate::pipeline::acquire_slot(&executor, global_config)?;

    if session.is_none()
        && let Ok(inherited_session_id) = std::env::var("CSA_SESSION_ID")
    {
        warn!(
            inherited_session_id = %inherited_session_id,
            "Ignoring inherited CSA_SESSION_ID for `csa review`; pass --session to resume explicitly"
        );
    }

    let execution = crate::pipeline::execute_with_session_and_meta_with_parent_source(
        &executor,
        &tool,
        &effective_prompt,
        OutputFormat::Json,
        session,
        Some(description),
        None,
        project_root,
        project_config,
        extra_env,
        Some("review"),
        None,
        None,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        None,
        None,
        Some(global_config),
        crate::pipeline::ParentSessionSource::ExplicitOnly,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
    )
    .await?;

    persist_review_routing_artifact(project_root, &execution.meta_session_id, &review_routing);

    Ok(execution)
}

#[cfg(test)]
#[path = "review_cmd_tests.rs"]
mod tests;
