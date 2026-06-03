use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use tracing::{info, warn};

use csa_core::types::{OutputFormat, ToolArg, ToolSelectionStrategy};
use csa_lock::SessionLock;
use csa_process::StreamMode;

use crate::pipeline;
use crate::run_cmd_caller_fork::resolve_fork_from_caller;
use crate::run_cmd_fork::try_auto_seed_fork;
use crate::run_cmd_model_pin::{
    RunModelPinInput, inherited_model_pin_from_startup, resolve_handle_run_model_pin,
};
use crate::run_cmd_post::{
    handle_fork_call_resume, mark_seed_and_evict, update_fork_genealogy,
    write_fallback_chain_to_result_toml,
};
use crate::run_cmd_tool_selection::{
    resolve_last_session_selection, resolve_return_target_session_id, resolve_skill_and_prompt,
    resolve_tool_by_strategy,
};
use crate::run_helpers::{
    apply_compound_tier_selector_arg, is_routing_conflict, resolve_positional_stdin_sentinel,
    resolve_prompt_with_file, resolve_task_edit_requirement, tier_bypass_allowed, truncate_prompt,
    warn_if_tier_without_tool,
};
use crate::run_helpers_branch_guard::{
    BranchGuardRuntime, evaluate_and_emit_refusal, observe_branch_state,
};
use crate::session_guard::{PreExecErrorCtx, persist_pre_exec_error_result};
use crate::startup_env::StartupSubtreeEnv;
#[path = "run_cmd_execute_post_exec_gate.rs"]
mod post_exec_gate;
#[path = "run_cmd_execute_reuse_hint.rs"]
mod reuse_hint;
#[path = "run_cmd_execute_routing.rs"]
mod routing;
#[path = "run_cmd_execute_cli_flags.rs"]
mod run_cli_flags;
#[path = "run_cmd_execute_context.rs"]
mod run_context;
#[path = "run_cmd_execute_output.rs"]
mod run_output;
#[path = "run_cmd_execute_skill_resume.rs"]
mod skill_resume;
#[path = "run_cmd_execute_tier_guard.rs"]
mod tier_guard;
use post_exec_gate::{
    PostExecGateApplyOptions, apply_post_exec_gate_after_success_with_runner,
    execute_post_exec_gate_command,
};
use reuse_hint::emit_reusable_session_hint;
use routing::{
    RunModelSelectionFlags, enforce_run_tier_bypass_gate, resolve_primary_writer_spec_for_run,
    resolve_run_effective_tier, resolve_run_no_failover, resolve_run_subtree_pin_selection,
    resolve_run_tier_context,
};
use run_cli_flags::{
    resolve_return_target, warn_deprecated_session_flags,
    warn_if_fast_mode_has_no_codex_run_candidate,
};
use run_context::finalize_prompt_text;
use run_output::emit_run_result_output;
use skill_resume::maybe_auto_resume_interrupted_skill_session;
use tier_guard::{DirectToolTierGuardCtx, enforce_direct_tool_tier_guard};

use super::attempt::{RunLoopCompletion, RunLoopRequest, execute_run_loop};
use super::resume::{
    detect_effective_repo, resolve_run_timeout_seconds, skill_session_description,
};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_run(
    tool: Option<ToolArg>,
    auto_route: Option<String>,
    hint_difficulty: Option<String>,
    skill: Option<String>,
    prompt: Option<String>,
    prompt_flag: Option<String>,
    prompt_file: Option<PathBuf>,
    inline_context_from_review_session: Option<String>,
    session_arg: Option<String>,
    last: bool,
    fork_from: Option<String>,
    fork_last: bool,
    fork_from_caller: bool,
    description: Option<String>,
    fork_call: bool,
    return_to: Option<String>,
    parent: Option<String>,
    ephemeral: bool,
    allow_base_branch_working: bool,
    cd: Option<String>,
    model_spec: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    force: bool,
    force_override_user_config: bool,
    allow_fallback: bool,
    no_failover: bool,
    fast_but_more_cost: bool,
    build_jobs: Option<u32>,
    wait: bool,
    idle_timeout: Option<u64>,
    initial_response_timeout: Option<u64>,
    timeout: Option<u64>,
    no_idle_timeout: bool,
    no_memory: bool,
    memory_query: Option<String>,
    current_depth: u32,
    output_format: OutputFormat,
    stream_mode: StreamMode,
    tier: Option<String>,
    force_ignore_tier_setting: bool,
    no_fs_sandbox: bool,
    no_error_marker_scan: bool,
    no_hook_bypass_scan: bool,
    no_preflight: bool,
    no_post_exec_gate: bool,
    require_commit: bool,
    extra_writable: Vec<PathBuf>,
    extra_readable: Vec<PathBuf>,
    startup_env: StartupSubtreeEnv,
) -> Result<i32> {
    let cli_model_spec_explicit = model_spec.is_some();
    let cli_model_explicit = model.is_some();
    let cli_thinking_explicit = thinking.is_some();
    let mut auto_route = auto_route;
    let mut model_spec = model_spec;
    let mut tier = tier;
    let mut force_ignore_tier_setting = force_ignore_tier_setting;
    let mut no_failover = no_failover;

    let project_root = pipeline::determine_project_root(cd.as_deref())?;
    let effective_repo =
        detect_effective_repo(&project_root).unwrap_or_else(|| "(unknown)".to_string());
    eprintln!(
        "csa run context: effective_repo={} effective_cwd={}",
        effective_repo,
        project_root.display()
    );

    warn_deprecated_session_flags(last, session_arg.is_some());
    let return_target = resolve_return_target(fork_call, return_to.as_deref())?;

    let mut is_fork = fork_from.is_some() || fork_last;
    let mut session_arg = if fork_last {
        info!("Resolving --fork-last to most recent session");
        let sessions = csa_session::list_sessions(&project_root, None)?;
        let (selected_id, ambiguity_warning) = resolve_last_session_selection(sessions)?;
        if let Some(warning) = ambiguity_warning {
            eprintln!("{warning}");
        }
        Some(selected_id)
    } else if fork_from.is_some() {
        info!(fork_from = ?fork_from, "Forking from specified session");
        fork_from
    } else if last {
        let sessions = csa_session::list_sessions(&project_root, None)?;
        let (selected_id, ambiguity_warning) = resolve_last_session_selection(sessions)?;
        if let Some(warning) = ambiguity_warning {
            eprintln!("{warning}");
        }
        Some(selected_id)
    } else {
        session_arg
    };

    if fork_call {
        let parent_session_id = resolve_return_target_session_id(
            return_target
                .as_ref()
                .expect("return target should be present for fork-call"),
            &project_root,
            session_arg.as_deref(),
            parent.as_deref(),
            startup_env.session_id(),
        )?;

        if session_arg.is_none() {
            if let Some(ref parent_id) = parent_session_id {
                session_arg = Some(parent_id.clone());
            } else {
                anyhow::bail!(
                    "fork-call requires a source session: provide --fork-from/--fork-last, \
                     or set --return-to/--parent/CSA_SESSION_ID"
                );
            }
        }

        is_fork = true;
    }

    let Some((mut config, mut global_config)) =
        pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };
    let caller_fork_resolution = if fork_from_caller {
        let resolved = resolve_fork_from_caller(config.as_ref());
        if resolved.is_none() {
            warn!("--fork-from-caller: no caller session resolved; falling back to cold start");
        }
        resolved
    } else {
        None
    };
    let branch_guard =
        BranchGuardRuntime::for_run(allow_base_branch_working, config.as_ref(), &global_config);
    let branch_state = observe_branch_state(&project_root, config.as_ref());
    if let Some(exit_code) = evaluate_and_emit_refusal(&branch_guard, branch_state) {
        return Ok(exit_code);
    }
    crate::run_cmd_preflight::apply_run_preflight_override(
        &project_root,
        session_arg.as_deref(),
        no_preflight,
        &mut config,
        &mut global_config,
    )?;
    let pre_session_hook = csa_hooks::load_global_pre_session_hook_invocation();
    let mut user_explicit_tool = tool.is_some();
    let prompt = resolve_positional_stdin_sentinel(prompt)?.or(prompt_flag);

    let prompt = if prompt_file.is_some() {
        Some(resolve_prompt_with_file(prompt, prompt_file.as_deref())?)
    } else {
        prompt
    };

    let mut skill_res = resolve_skill_and_prompt(
        skill.as_deref(),
        prompt,
        tool,
        model,
        thinking,
        &project_root,
    )?;
    let inherited_model_pin = inherited_model_pin_from_startup(&startup_env);
    let model_pin_resolution = resolve_handle_run_model_pin(
        RunModelPinInput {
            model_spec,
            tier,
            auto_route,
            force_ignore_tier_setting,
            no_failover,
        },
        inherited_model_pin.clone(),
        cli_model_spec_explicit,
        &mut skill_res,
        &mut user_explicit_tool,
    );
    model_spec = model_pin_resolution.model_spec;
    tier = model_pin_resolution.tier;
    auto_route = model_pin_resolution.auto_route;
    force_ignore_tier_setting = model_pin_resolution.force_ignore_tier_setting;
    no_failover = model_pin_resolution.no_failover;
    let resolved_skill = skill_res.resolved_skill;
    let gate_prompt_text = skill_res.prompt_text.clone();
    let frontmatter_difficulty = skill_res.frontmatter_difficulty.clone();
    let task_needs_edit =
        resolve_task_edit_requirement(resolved_skill.as_ref(), &skill_res.prompt_text);
    let prompt_text = finalize_prompt_text(
        &project_root,
        skill_res.prompt_text,
        inline_context_from_review_session.as_deref(),
        &startup_env,
    )?;
    let skill_agent = resolved_skill.as_ref().and_then(|sk| sk.agent_config());
    let thinking = skill_res.thinking;
    let model = skill_res.model;
    let skill_session_tag = skill.as_deref().map(skill_session_description);
    let model_selection_flags = RunModelSelectionFlags {
        tool: user_explicit_tool,
        auto_route: auto_route.is_some(),
        skill: skill.is_some(),
        model_spec: model_spec.is_some(),
        model: model.is_some(),
        thinking: thinking.is_some(),
        cli_model: cli_model_explicit,
        cli_thinking: cli_thinking_explicit,
        tier: tier.is_some(),
        hint_difficulty: hint_difficulty.is_some() || frontmatter_difficulty.is_some(),
    };
    enforce_run_tier_bypass_gate(
        config.as_ref(),
        &global_config,
        model_selection_flags,
        force,
        force_ignore_tier_setting,
        model_pin_resolution.inherited_trusted_pin,
    )?;
    if model_selection_flags.model_spec
        && tier_bypass_allowed(
            config.as_ref(),
            &global_config,
            model_pin_resolution.inherited_trusted_pin,
        )
    {
        force_ignore_tier_setting = true;
    }
    let primary_writer_spec =
        resolve_primary_writer_spec_for_run(model_selection_flags, config.as_ref(), &global_config);
    let model_spec = model_spec.or(primary_writer_spec);

    let mut merged_aliases = global_config.tool_aliases.clone();
    if let Some(c) = config.as_ref() {
        merged_aliases.extend(c.tool_aliases.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    let explicit_tool_name = match skill_res.tool.as_ref() {
        Some(ToolArg::Specific(tool)) => Some(tool.as_str()),
        _ => None,
    };
    let fallback_description = truncate_prompt(&prompt_text, 80);
    let pre_exec_description = description
        .as_deref()
        .or(skill_session_tag.as_deref())
        .or(Some(fallback_description.as_str()));
    let pre_exec_parent = if is_fork {
        session_arg.as_deref().or(parent.as_deref())
    } else {
        parent.as_deref()
    };

    let effective_tier = resolve_run_effective_tier(
        config.as_ref(),
        tier.as_deref(),
        auto_route.as_deref(),
        model_spec.as_deref(),
        hint_difficulty.as_deref(),
        frontmatter_difficulty.as_deref(),
    )?;

    let (effective_tier, compounded_tool) = match apply_compound_tier_selector_arg(
        effective_tier,
        skill_res.tool.take(),
        config.as_ref(),
    ) {
        Ok(pair) => pair,
        Err(err) => {
            return Err(persist_pre_exec_error_result(PreExecErrorCtx {
                project_root: &project_root,
                session_id: if is_fork {
                    None
                } else {
                    session_arg.as_deref()
                },
                description: pre_exec_description,
                parent: pre_exec_parent,
                tool_name: explicit_tool_name,
                task_type: Some("run"),
                tier_name: None,
                error: err,
            }));
        }
    };
    skill_res.tool = compounded_tool;

    enforce_direct_tool_tier_guard(DirectToolTierGuardCtx {
        config: config.as_ref(),
        user_explicit_tool,
        effective_tier: effective_tier.as_deref(),
        model_spec: model_spec.as_deref(),
        force_ignore_tier_setting,
        force,
        project_root: &project_root,
        is_fork,
        session_arg: session_arg.as_deref(),
        pre_exec_description,
        pre_exec_parent,
        explicit_tool_name,
    })?;

    warn_if_tier_without_tool(tier.as_deref(), user_explicit_tool);

    let strategy = skill_res
        .tool
        .unwrap_or(ToolArg::Auto)
        .resolve_alias(&merged_aliases)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .into_strategy();
    let effective_no_failover = resolve_run_no_failover(
        user_explicit_tool,
        effective_tier.is_some(),
        &strategy,
        no_failover,
        allow_fallback,
    );
    let run_timeout_seconds = resolve_run_timeout_seconds(timeout, skill.as_deref());
    let idle_timeout_seconds = if no_idle_timeout {
        info!("Idle timeout disabled via --no-idle-timeout");
        u64::MAX
    } else {
        pipeline::resolve_effective_idle_timeout_seconds(config.as_ref(), idle_timeout, timeout)
    };
    let run_started_at = Instant::now();
    let needs_edit = task_needs_edit.unwrap_or(true);
    let strategy_result = resolve_tool_by_strategy(
        &strategy,
        model_spec.as_deref(),
        model.as_deref(),
        thinking.as_deref(),
        config.as_ref(),
        &global_config,
        &project_root,
        force,
        force_override_user_config,
        needs_edit,
        effective_tier.as_deref(),
        force_ignore_tier_setting,
    )
    .map_err(|err| {
        if is_routing_conflict(&err) {
            persist_pre_exec_error_result(PreExecErrorCtx {
                project_root: &project_root,
                session_id: if is_fork {
                    None
                } else {
                    session_arg.as_deref()
                },
                description: pre_exec_description,
                parent: pre_exec_parent,
                tool_name: explicit_tool_name,
                task_type: Some("run"),
                tier_name: effective_tier.as_deref(),
                error: err,
            })
        } else {
            err
        }
    })?;
    let heterogeneous_runtime_fallback_candidates = strategy_result.runtime_fallback_candidates;
    let resolved_model_spec = strategy_result.model_spec;
    let resolved_model = strategy_result.model;
    let strategy_resolved_tier_name = strategy_result.resolved_tier_name;
    let resolved_tool = strategy_result.tool;
    let subtree_model_pin_selection = resolve_run_subtree_pin_selection(
        model_pin_resolution.subtree_model_pin_active,
        model_spec.as_deref(),
        user_explicit_tool,
        effective_tier.is_some(),
        resolved_model_spec.as_deref(),
    );
    warn_if_fast_mode_has_no_codex_run_candidate(
        fast_but_more_cost,
        resolved_tool,
        &heterogeneous_runtime_fallback_candidates,
    );
    session_arg = maybe_auto_resume_interrupted_skill_session(
        &project_root,
        skill.as_deref(),
        &resolved_tool,
        session_arg,
        is_fork,
        fork_call,
        ephemeral,
    );

    let seed_result = try_auto_seed_fork(
        &project_root,
        &resolved_tool,
        config.as_ref(),
        is_fork,
        session_arg,
        ephemeral,
    );
    let is_auto_seed_fork = seed_result.is_auto_seed_fork;
    is_fork = seed_result.is_fork;
    session_arg = seed_result.session_arg;

    let mut _fork_call_parent_lock: Option<SessionLock> = None;
    let mut fork_call_parent_session_id: Option<String> = None;
    if fork_call {
        let resolved_parent_id = resolve_return_target_session_id(
            return_target
                .as_ref()
                .expect("return target should be present for fork-call"),
            &project_root,
            session_arg.as_deref(),
            parent.as_deref(),
            startup_env.session_id(),
        )?;
        let Some(parent_id) = resolved_parent_id else {
            anyhow::bail!("unable to resolve parent session for fork-call return");
        };

        let state_root = csa_session::get_session_root(&project_root)?;
        _fork_call_parent_lock = Some(csa_lock::acquire_parent_fork_lock(
            &state_root,
            &parent_id,
            "fork-call parent serialization",
        )?);

        let mut parent_state = csa_session::load_session(&project_root, &parent_id)?;
        parent_state
            .record_fork_call_attempt(Instant::now())
            .map_err(anyhow::Error::msg)?;
        csa_session::save_session(&parent_state)?;
        fork_call_parent_session_id = Some(parent_id.clone());

        if session_arg.is_none() {
            session_arg = Some(parent_id);
            is_fork = true;
        }
    }

    let effective_session_arg = if is_fork { None } else { session_arg.clone() };

    emit_reusable_session_hint(
        &project_root,
        resolved_tool,
        effective_session_arg.as_deref(),
        is_fork,
    );

    let fallback_tier_name = skill_agent.and_then(|a| a.tier.clone()).or_else(|| {
        config.as_ref().and_then(|cfg| {
            cfg.tier_mapping.get("default").cloned().or_else(|| {
                if cfg.tiers.contains_key("tier3") {
                    Some("tier3".to_string())
                } else {
                    cfg.tiers.keys().next().cloned()
                }
            })
        })
    });
    let user_model_spec_explicit = model_spec.is_some();
    let (tier_auto_select, failover_on_crash_enabled, resolved_tier_name) =
        resolve_run_tier_context(
            config.as_ref(),
            resolved_tool.as_str(),
            strategy_resolved_tier_name,
            fallback_tier_name,
            force_ignore_tier_setting,
            user_model_spec_explicit,
            user_explicit_tool,
        );
    let context_load_options = skill_agent
        .and_then(|agent| pipeline::context_load_options_with_skips(&agent.skip_context));
    let memory_injection = pipeline::MemoryInjectionOptions {
        disabled: no_memory,
        query_override: memory_query,
    };

    let loop_strategy = if user_model_spec_explicit {
        ToolSelectionStrategy::Explicit(resolved_tool)
    } else {
        strategy
    };
    let loop_completion = execute_run_loop(RunLoopRequest {
        strategy: loop_strategy,
        initial_tool: resolved_tool,
        initial_model_spec: resolved_model_spec,
        user_model_spec_explicit: false,
        subtree_model_pin_spec: subtree_model_pin_selection.model_spec.as_deref(),
        subtree_model_pin_force_ignore_tier_setting: subtree_model_pin_selection
            .force_ignore_tier_setting,
        initial_model: resolved_model,
        runtime_fallback_candidates: heterogeneous_runtime_fallback_candidates,
        project_root: &project_root,
        config: config.as_ref(),
        global_config: &global_config,
        prompt_text: &prompt_text,
        skill: skill.as_deref(),
        skill_session_tag,
        description,
        parent,
        output_format,
        stream_mode,
        thinking: thinking.as_deref(),
        force,
        force_override_user_config,
        force_ignore_tier_setting,
        no_failover: effective_no_failover,
        fast_but_more_cost,
        build_jobs,
        wait,
        idle_timeout_seconds,
        cli_idle_timeout: idle_timeout,
        cli_initial_response_timeout: initial_response_timeout,
        no_idle_timeout,
        run_timeout_seconds,
        run_started_at,
        is_fork,
        is_auto_seed_fork,
        caller_fork_resolution,
        ephemeral,
        fork_call,
        session_arg,
        effective_session_arg,
        tier_auto_select,
        failover_on_crash_enabled,
        resolved_tier_name: resolved_tier_name.as_deref(),
        context_load_options: context_load_options.as_ref(),
        memory_injection,
        pre_session_hook,
        task_needs_edit,
        no_fs_sandbox,
        no_error_marker_scan,
        no_hook_bypass_scan,
        extra_writable,
        extra_readable,
        branch_guard,
        startup_env: &startup_env,
    })
    .await?;

    let loop_outcome = match loop_completion {
        RunLoopCompletion::Exit(exit_code) => return Ok(exit_code),
        RunLoopCompletion::Completed(loop_outcome) => *loop_outcome,
    };
    let mut result = loop_outcome.result;
    let current_tool = loop_outcome.current_tool;
    let executed_session_id = loop_outcome.executed_session_id;
    let changed_paths = loop_outcome.changed_paths;
    let fork_resolution = loop_outcome.fork_resolution;
    super::uncommitted::record_run_dirty(
        &project_root,
        executed_session_id.as_deref(),
        &mut result,
        require_commit,
        config.as_ref(),
    );

    if result.exit_code == 0 {
        let post_exec_gate_env = crate::build_jobs_env::build_jobs_env(build_jobs);
        apply_post_exec_gate_after_success_with_runner(
            &project_root,
            &gate_prompt_text,
            executed_session_id.as_deref(),
            config.as_ref(),
            PostExecGateApplyOptions {
                changed_paths: changed_paths.as_deref(),
                extra_env: post_exec_gate_env,
                no_post_exec_gate,
            },
            execute_post_exec_gate_command,
        )
        .await?;
    }

    if fork_call {
        let parent_session_id = fork_call_parent_session_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("fork-call parent session is unresolved"))?;
        handle_fork_call_resume(
            &project_root,
            executed_session_id.as_deref(),
            &parent_session_id,
            &current_tool,
            return_target.is_some(),
            config.as_ref(),
            &global_config,
        )?;
    }

    if let Some(ref fork_res) = fork_resolution
        && let Some(ref sid) = executed_session_id
    {
        update_fork_genealogy(&project_root, sid, fork_res, &current_tool);
    }

    if result.exit_code == 0
        && fork_resolution.is_none()
        && !ephemeral
        && let Some(ref sid) = executed_session_id
    {
        mark_seed_and_evict(&project_root, sid, &current_tool, config.as_ref());
    }

    if !loop_outcome.fallback_chain.is_empty()
        && let Some(ref sid) = executed_session_id
    {
        write_fallback_chain_to_result_toml(&project_root, sid, &loop_outcome.fallback_chain);
    }

    emit_run_result_output(
        &project_root,
        output_format,
        executed_session_id.as_deref(),
        &result,
    )?;

    Ok(result.exit_code)
}

#[cfg(test)]
#[path = "run_cmd_execute_pre_exec_tests.rs"]
mod pre_exec_tests;

#[cfg(test)]
#[path = "run_cmd_execute_tests.rs"]
mod tests;
