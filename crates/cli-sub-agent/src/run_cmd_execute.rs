//! Top-level `csa run` command orchestration.
//!
//! Extracted from `run_cmd.rs` to keep module sizes manageable.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use tracing::{info, warn};

use csa_config::ProjectConfig;
use csa_core::types::{OutputFormat, ToolArg, ToolSelectionStrategy};
use csa_lock::SessionLock;

use crate::cli::ReturnTarget;
use crate::pipeline;
use crate::run_cmd_fork::try_auto_seed_fork;
use crate::run_cmd_post::{handle_fork_call_resume, mark_seed_and_evict, update_fork_genealogy};
use crate::run_cmd_tool_selection::{
    resolve_last_session_selection, resolve_return_target_session_id, resolve_skill_and_prompt,
    resolve_tool_by_strategy,
};

use super::attempt::{RunLoopCompletion, RunLoopRequest, execute_run_loop};
use super::resume::{
    detect_effective_repo, find_recent_interrupted_skill_session, resolve_run_timeout_seconds,
    skill_session_description,
};

fn resolve_run_tier_context(
    config: Option<&ProjectConfig>,
    tool_name: &str,
    strategy_resolved_tier_name: Option<String>,
    fallback_tier_name: Option<String>,
    force_ignore_tier_setting: bool,
    user_model_spec_explicit: bool,
    user_explicit_tool: bool,
) -> (bool, bool, Option<String>) {
    if force_ignore_tier_setting || user_model_spec_explicit {
        return (false, false, None);
    }

    let resolved_tier_name = strategy_resolved_tier_name.or_else(|| {
        (!user_explicit_tool)
            .then_some(fallback_tier_name)
            .flatten()
    });
    let tier_auto_select = !user_explicit_tool && resolved_tier_name.is_some();
    let failover_on_crash_enabled = tier_auto_select
        || (user_explicit_tool
            && resolved_tier_name.as_deref().is_some_and(|tier_name| {
                config.is_some_and(|cfg| cfg.tier_contains_tool(tier_name, tool_name))
            }));

    (
        tier_auto_select,
        failover_on_crash_enabled,
        resolved_tier_name,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_run(
    tool: Option<csa_core::types::ToolArg>,
    auto_route: Option<String>,
    skill: Option<String>,
    prompt: Option<String>,
    prompt_flag: Option<String>,
    prompt_file: Option<PathBuf>,
    session_arg: Option<String>,
    last: bool,
    fork_from: Option<String>,
    fork_last: bool,
    description: Option<String>,
    fork_call: bool,
    return_to: Option<String>,
    parent: Option<String>,
    ephemeral: bool,
    cd: Option<String>,
    model_spec: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    force: bool,
    force_override_user_config: bool,
    no_failover: bool,
    wait: bool,
    idle_timeout: Option<u64>,
    initial_response_timeout: Option<u64>,
    timeout: Option<u64>,
    no_idle_timeout: bool,
    no_memory: bool,
    memory_query: Option<String>,
    current_depth: u32,
    output_format: OutputFormat,
    stream_mode: csa_process::StreamMode,
    tier: Option<String>,
    force_ignore_tier_setting: bool,
    no_fs_sandbox: bool,
    extra_writable: Vec<PathBuf>,
    extra_readable: Vec<PathBuf>,
) -> Result<i32> {
    let project_root = pipeline::determine_project_root(cd.as_deref())?;
    let effective_repo =
        detect_effective_repo(&project_root).unwrap_or_else(|| "(unknown)".to_string());
    eprintln!(
        "csa run context: effective_repo={} effective_cwd={}",
        effective_repo,
        project_root.display()
    );

    if last {
        warn!("--last is deprecated: use --fork-last instead (fork-first architecture)");
        eprintln!(
            "warning: --last is deprecated and will be removed in a future release. Use --fork-last instead."
        );
    }
    if session_arg.is_some() {
        warn!("--session is deprecated: use --fork-from instead (fork-first architecture)");
        eprintln!(
            "warning: --session is deprecated and will be removed in a future release. Use --fork-from instead."
        );
    }

    let return_target = if fork_call {
        Some(match return_to.as_deref() {
            Some(value) => crate::cli::parse_return_to(value)?,
            None => ReturnTarget::Auto,
        })
    } else {
        None
    };

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

    let Some((config, global_config)) = pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };
    let effective_tier = tier.or(auto_route.clone());

    // Track whether user explicitly provided --tool on the CLI (before skill
    // resolution may override it).  This drives tier enforcement: explicit
    // --tool (including --tool auto) is blocked when tiers are configured.
    let user_explicit_tool = tool.is_some();
    let prompt = prompt.or(prompt_flag);

    // Resolve --prompt-file into the prompt if provided.
    let prompt = if prompt_file.is_some() {
        Some(crate::run_helpers::resolve_prompt_with_file(
            prompt,
            prompt_file.as_deref(),
        )?)
    } else {
        prompt
    };

    let skill_res = resolve_skill_and_prompt(
        skill.as_deref(),
        prompt,
        tool,
        model,
        thinking,
        &project_root,
    )?;
    let prompt_text = skill_res.prompt_text;
    let resolved_skill = skill_res.resolved_skill;
    let skill_agent = resolved_skill.as_ref().and_then(|sk| sk.agent_config());
    let thinking = skill_res.thinking;
    let model = skill_res.model;
    let skill_session_tag = skill.as_deref().map(skill_session_description);

    let mut merged_aliases = global_config.tool_aliases.clone();
    if let Some(c) = config.as_ref() {
        merged_aliases.extend(c.tool_aliases.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    let explicit_tool_name = match skill_res.tool.as_ref() {
        Some(ToolArg::Specific(tool)) => Some(tool.as_str()),
        _ => None,
    };
    let fallback_description = crate::run_helpers::truncate_prompt(&prompt_text, 80);
    let pre_exec_description = description
        .as_deref()
        .or(skill_session_tag.as_deref())
        .or(Some(fallback_description.as_str()));
    let pre_exec_parent = if is_fork {
        session_arg.as_deref().or(parent.as_deref())
    } else {
        parent.as_deref()
    };

    // Enforce tier routing: when tiers are configured, explicit --tool (any
    // value, including "auto") is blocked unless --tier is also specified or
    // --force-ignore-tier-setting is active.
    let tiers_configured = config.as_ref().is_some_and(|c| !c.tiers.is_empty());
    if user_explicit_tool
        && tiers_configured
        && effective_tier.is_none()
        && !force_ignore_tier_setting
        && !force
    {
        let cfg = config.as_ref().unwrap();
        let tier_list: Vec<&str> = cfg.tiers.keys().map(|s| s.as_str()).collect();
        let err = anyhow::anyhow!(
            "Direct --tool is blocked when tiers are configured.\n\
             Use --tier <name> or --auto-route <intent> to select tier-based routing, or \
             --force-ignore-tier-setting to bypass.\n\
             Available tiers: {}",
            tier_list.join(", ")
        );
        return Err(crate::session_guard::persist_pre_exec_error_result(
            crate::session_guard::PreExecErrorCtx {
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
            },
        ));
    }

    let strategy = skill_res
        .tool
        .unwrap_or(ToolArg::Auto)
        .resolve_alias(&merged_aliases)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .into_strategy();
    let idle_timeout_seconds = if no_idle_timeout {
        info!("Idle timeout disabled via --no-idle-timeout");
        u64::MAX
    } else {
        pipeline::resolve_idle_timeout_seconds(config.as_ref(), idle_timeout)
    };
    let run_timeout_seconds = resolve_run_timeout_seconds(timeout, skill.as_deref());
    let run_started_at = Instant::now();
    let task_needs_edit =
        crate::run_helpers::resolve_task_edit_requirement(resolved_skill.as_ref(), &prompt_text);
    let needs_edit = task_needs_edit.unwrap_or(false);
    let strategy_result = resolve_tool_by_strategy(
        &strategy,
        model_spec.as_deref(),
        model.as_deref(),
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
        if crate::run_helpers::is_routing_conflict(&err) {
            crate::session_guard::persist_pre_exec_error_result(
                crate::session_guard::PreExecErrorCtx {
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
                },
            )
        } else {
            err
        }
    })?;
    let heterogeneous_runtime_fallback_candidates = strategy_result.runtime_fallback_candidates;
    let resolved_model_spec = strategy_result.model_spec;
    let resolved_model = strategy_result.model;
    let strategy_resolved_tier_name = strategy_result.resolved_tier_name;
    let resolved_tool = strategy_result.tool;
    if session_arg.is_none()
        && !is_fork
        && !fork_call
        && !ephemeral
        && let Some(skill_name) = skill.as_deref()
        && let Some(interrupted_session_id) =
            find_recent_interrupted_skill_session(&project_root, skill_name, &resolved_tool)
    {
        eprintln!(
            "Auto-resuming interrupted skill session {interrupted_session_id} for '{skill_name}'."
        );
        session_arg = Some(interrupted_session_id);
    }

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

    if effective_session_arg.is_none() && !is_fork {
        let tool_names = vec![resolved_tool.as_str().to_string()];
        match csa_scheduler::session_reuse::find_reusable_sessions(
            &project_root,
            "run",
            &tool_names,
        ) {
            Ok(candidates) if !candidates.is_empty() => {
                let best = &candidates[0];
                eprintln!(
                    "hint: reusable session available for {}: --fork-from {}",
                    best.tool_name,
                    best.session_id.get(..8).unwrap_or(&best.session_id),
                );
            }
            _ => {}
        }
    }

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
    // Force-ignore bypass must not revive a tier at runtime, but ordinary
    // auto/default routing still keeps its existing fallback tier context.
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
        user_model_spec_explicit,
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
        no_failover,
        wait,
        idle_timeout_seconds,
        cli_idle_timeout: idle_timeout,
        cli_initial_response_timeout: initial_response_timeout,
        no_idle_timeout,
        run_timeout_seconds,
        run_started_at,
        is_fork,
        is_auto_seed_fork,
        ephemeral,
        fork_call,
        session_arg,
        effective_session_arg,
        tier_auto_select,
        failover_on_crash_enabled,
        resolved_tier_name: resolved_tier_name.as_deref(),
        context_load_options: context_load_options.as_ref(),
        memory_injection,
        task_needs_edit,
        no_fs_sandbox,
        extra_writable,
        extra_readable,
    })
    .await?;

    let loop_outcome = match loop_completion {
        RunLoopCompletion::Exit(exit_code) => return Ok(exit_code),
        RunLoopCompletion::Completed(loop_outcome) => *loop_outcome,
    };
    let result = loop_outcome.result;
    let current_tool = loop_outcome.current_tool;
    let executed_session_id = loop_outcome.executed_session_id;
    let fork_resolution = loop_outcome.fork_resolution;

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

    match output_format {
        OutputFormat::Text => {
            print!("{}", result.output);
            if result.exit_code != 0
                && let Some(ref sid) = executed_session_id
                && let Some(hint) = crate::error_hints::sandbox_fs_denial_hint(
                    &result.stderr_output,
                    &result.output,
                    sid,
                )
            {
                eprintln!("{hint}");
            }
        }
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&result)?;
            println!("{json}");
        }
    }

    Ok(result.exit_code)
}

#[cfg(test)]
#[path = "run_cmd_execute_pre_exec_tests.rs"]
mod pre_exec_tests;

#[cfg(test)]
mod tests {
    use super::resolve_run_tier_context;
    use chrono::Utc;
    use csa_config::{ProjectConfig, ProjectMeta, TierConfig, TierStrategy};
    use std::collections::HashMap;

    fn make_test_config() -> ProjectConfig {
        let mut tiers = HashMap::new();
        tiers.insert(
            "tier-3-complex".to_string(),
            TierConfig {
                description: "test".to_string(),
                models: vec![
                    "codex/openai/o4-mini/high".to_string(),
                    "claude-code/anthropic/claude-sonnet/high".to_string(),
                ],
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        );

        ProjectConfig {
            schema_version: 1,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: Default::default(),
            acp: Default::default(),
            tools: HashMap::new(),
            review: None,
            debate: None,
            tiers,
            tier_mapping: HashMap::from([("default".to_string(), "tier-3-complex".to_string())]),
            aliases: HashMap::new(),
            tool_aliases: HashMap::new(),
            preferences: None,
            session: Default::default(),
            memory: Default::default(),
            hooks: Default::default(),
            execution: Default::default(),
            preflight: Default::default(),
            vcs: Default::default(),
            filesystem_sandbox: Default::default(),
        }
    }

    #[test]
    fn resolve_run_tier_context_keeps_active_strategy_tier() {
        let (tier_auto_select, failover_on_crash_enabled, resolved_tier_name) =
            resolve_run_tier_context(
                None,
                "codex",
                Some("tier-3-complex".to_string()),
                None,
                false,
                false,
                false,
            );

        assert!(tier_auto_select);
        assert!(failover_on_crash_enabled);
        assert_eq!(resolved_tier_name.as_deref(), Some("tier-3-complex"));
    }

    #[test]
    fn resolve_run_tier_context_drops_bypassed_tier() {
        let (tier_auto_select, failover_on_crash_enabled, resolved_tier_name) =
            resolve_run_tier_context(
                None,
                "codex",
                Some("tier-3-complex".to_string()),
                Some("tier-2-standard".to_string()),
                true,
                false,
                true,
            );

        assert!(!tier_auto_select);
        assert!(!failover_on_crash_enabled);
        assert!(resolved_tier_name.is_none());
    }

    #[test]
    fn resolve_run_tier_context_restores_fallback_tier_for_auto_routing() {
        let (tier_auto_select, failover_on_crash_enabled, resolved_tier_name) =
            resolve_run_tier_context(
                None,
                "codex",
                None,
                Some("tier-3-complex".to_string()),
                false,
                false,
                false,
            );

        assert!(tier_auto_select);
        assert!(failover_on_crash_enabled);
        assert_eq!(resolved_tier_name.as_deref(), Some("tier-3-complex"));
    }

    #[test]
    fn resolve_run_tier_context_does_not_restore_fallback_for_user_explicit_tool() {
        let (tier_auto_select, failover_on_crash_enabled, resolved_tier_name) =
            resolve_run_tier_context(
                None,
                "codex",
                None,
                Some("tier-3-complex".to_string()),
                false,
                false,
                true,
            );

        assert!(!tier_auto_select);
        assert!(!failover_on_crash_enabled);
        assert!(resolved_tier_name.is_none());
    }

    #[test]
    fn resolve_run_tier_context_enables_crash_failover_for_explicit_tool_in_tier() {
        let config = make_test_config();
        let (tier_auto_select, failover_on_crash_enabled, resolved_tier_name) =
            resolve_run_tier_context(
                Some(&config),
                "codex",
                Some("tier-3-complex".to_string()),
                None,
                false,
                false,
                true,
            );

        assert!(!tier_auto_select);
        assert!(failover_on_crash_enabled);
        assert_eq!(resolved_tier_name.as_deref(), Some("tier-3-complex"));
    }

    #[test]
    fn resolve_run_tier_context_drops_tier_for_explicit_model_spec() {
        let (tier_auto_select, failover_on_crash_enabled, resolved_tier_name) =
            resolve_run_tier_context(
                None,
                "codex",
                None,
                Some("tier-3-complex".to_string()),
                false,
                true,
                false,
            );

        assert!(!tier_auto_select);
        assert!(!failover_on_crash_enabled);
        assert!(resolved_tier_name.is_none());
    }
}
