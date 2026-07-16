use anyhow::Result;
use serde::Serialize;
use tracing::warn;

use crate::cli::DebateArgs;
use crate::debate_cmd_resolve::{
    DebateTierResolveCtx, resolve_debate_effective_tier_with_compound, resolve_debate_model,
    resolve_debate_selection, resolve_debate_tier_name,
    validate_debate_direct_tool_tier_restriction,
};
use crate::startup_env::StartupSubtreeEnv;
use csa_core::types::{OutputFormat, ToolArg, ToolName};

#[cfg(test)]
use crate::tier_model_fallback::TierAttemptFailure;

#[path = "debate_cmd_subtree_pin.rs"]
mod subtree_pin;

#[path = "debate_cmd_question.rs"]
mod question;

#[path = "debate_cmd_finalize.rs"]
mod finalize;
pub(crate) use finalize::{DebateFinalizeContext, finalize_debate_outcome_with_catalog};
#[cfg(test)]
pub(crate) use finalize::{finalize_debate_outcome, resolve_persisted_debate_session_id};

#[path = "debate_cmd_execute.rs"]
mod execute;
use execute::{DebateExecutionRequest, execute_debate};

#[path = "debate_cmd_dry_run.rs"]
mod dry_run;

#[path = "debate_cmd_fast_mode.rs"]
mod fast_mode;

#[path = "debate_cmd_gate.rs"]
mod gate;
use gate::run_pre_debate_quality_gate;

#[path = "debate_cmd_readonly.rs"]
mod readonly;
#[cfg(test)]
use readonly::build_debate_instruction;
use readonly::build_debate_instruction_for_project;
pub(crate) use readonly::with_readonly_session_env;

#[path = "debate_cmd_runtime.rs"]
mod runtime;
#[cfg(test)]
use runtime::STILL_WORKING_BACKOFF;
#[cfg(test)]
use runtime::{
    ensure_debate_wall_clock_within_timeout, should_retry_debate_after_error,
    wait_for_still_working_backoff,
};
use runtime::{
    render_debate_cli_output, resolve_debate_stream_mode, resolve_debate_thinking,
    resolve_debate_timeout_seconds, verify_debate_skill_available,
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
    startup_env: &StartupSubtreeEnv,
) -> Result<i32> {
    // 1. Determine project root
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;

    // 2. Load config and validate recursion depth
    let Some((config, global_config, model_catalog, _project_completion_policy)) =
        crate::pipeline::load_and_validate(&project_root, current_depth)?
    else {
        return Ok(1);
    };
    // #1741: honor a pinned SA subtree's inherited model spec for `csa debate`
    // (see debate_cmd_subtree_pin::apply_subtree_pin).
    let inherited_model_pin =
        crate::run_cmd_model_pin::inherited_model_pin_from_startup(startup_env);
    let inherited_trusted_pin = subtree_pin::apply_subtree_pin(&mut args, inherited_model_pin);
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
    })
    .map_err(|err| {
        crate::session_guard::persist_pre_exec_error_result(crate::session_guard::PreExecErrorCtx {
            project_root: &project_root,
            session_id: args.session.as_deref(),
            description: Some("debate"),
            parent: None,
            tool_name: match args.tool.as_ref() {
                Some(ToolArg::Specific(tool)) => Some(tool.as_str()),
                Some(ToolArg::Auto | ToolArg::AnyAvailable | ToolArg::Alias(_)) | None => None,
            },
            task_type: Some("debate"),
            tier_name: args.tier.as_deref(),
            error: err,
        })
    })?;
    let pre_session_hook = csa_hooks::load_global_pre_session_hook_invocation();

    // 2b. Verify debate skill is available (fail fast before any execution)
    let debate_pattern = verify_debate_skill_available(&project_root)?;

    // 2c. Run pre-debate quality gate (reuses [review] gate settings)
    //
    // Debate reuses the review section's gate settings because the gate is a
    // shared pre-execution quality check (lint/test) that applies equally to
    // both review and debate workflows.
    if !args.dry_run {
        run_pre_debate_quality_gate(
            &project_root,
            config.as_ref(),
            &global_config,
            current_depth,
        )
        .await?;
    }

    // 3. Read question (positional / --topic / --question-file / stdin), strip
    // difficulty frontmatter, prepend --context / --file (see
    // debate_cmd_question::build_debate_question).
    let (question, frontmatter_difficulty) = question::build_debate_question(&mut args)?;

    // 4. Build debate instruction with the resolved pattern injected.
    let mut prompt = build_debate_instruction_for_project(
        &question,
        args.session.is_some(),
        args.rounds,
        &project_root,
        &debate_pattern,
    );
    if let Some(guard) =
        crate::pipeline::prompt_guard::anti_recursion_guard(config.as_ref(), current_depth)
    {
        prompt = format!("{guard}\n\n{prompt}");
    }
    let debate_description = format!(
        "debate: {}",
        crate::run_helpers::truncate_prompt(&question, 80)
    );

    // 5. Determine tool (with tier-based resolution)
    let detected_parent_tool = crate::run_helpers::detect_parent_tool();
    let parent_tool = crate::run_helpers::resolve_tool(detected_parent_tool, &global_config);
    let mut merged_tool_aliases = global_config.tool_aliases.clone();
    if let Some(c) = config.as_ref() {
        merged_tool_aliases.extend(c.tool_aliases.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    let cli_tool = resolve_debate_cli_tool(args.tool.take(), &merged_tool_aliases)?;
    let explicit_tool = cli_tool.or_else(|| {
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
            cli_tool,
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
        &model_catalog,
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
    let tier_preference_order = resolved_selection.tier_preference_order.clone();
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
    let readonly_project_root = global_config.debate.readonly_sandbox.unwrap_or(false);
    execute_debate(DebateExecutionRequest {
        args: &args,
        output_format,
        project_root: &project_root,
        config: config.as_ref(),
        global_config: &global_config,
        model_catalog: &model_catalog,
        pre_session_hook,
        prompt: &prompt,
        debate_description: &debate_description,
        tool,
        debate_mode,
        resolved_model_spec: resolved_model_spec.as_deref(),
        resolved_tier_name: resolved_tier_name.as_deref(),
        tier_active,
        tier_preference_order: &tier_preference_order,
        debate_model: debate_model.as_deref(),
        thinking: thinking.as_deref(),
        stream_mode,
        timeout_seconds,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        readonly_project_root,
        startup_env,
    })
    .await
}

fn resolve_debate_cli_tool(
    tool: Option<ToolArg>,
    tool_aliases: &std::collections::HashMap<String, String>,
) -> Result<Option<ToolName>> {
    match tool.unwrap_or(ToolArg::Auto).resolve_alias(tool_aliases) {
        Ok(ToolArg::Auto | ToolArg::AnyAvailable) => Ok(None),
        Ok(ToolArg::Specific(tool)) => Ok(Some(tool)),
        Ok(ToolArg::Alias(alias)) => {
            anyhow::bail!("BUG: unresolved debate --tool alias '{alias}' after alias resolution")
        }
        Err(err) => anyhow::bail!("{err}"),
    }
}

#[cfg(test)]
#[path = "debate_cmd_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "debate_cmd_resource_override_tests.rs"]
mod resource_override_tests;

#[cfg(test)]
#[path = "debate_cmd_readonly_tests.rs"]
mod readonly_tests;

#[cfg(test)]
#[path = "debate_cmd_question_tests.rs"]
mod question_tests;

#[cfg(test)]
#[path = "debate_cmd_round4_tests.rs"]
mod round4_tests;
