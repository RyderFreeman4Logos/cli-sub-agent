use anyhow::{Context, Result};
use csa_config::{ExecutionEnvOptions, GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_resource::{ResourceGuard, ResourceLimits};

use crate::cli::ReviewArgs;
use crate::run_resource_overrides::RunResourceOverrides;
use crate::startup_env::StartupSubtreeEnv;

const REVIEW_PREFLIGHT_SESSION_ID: &str = "review-pre-session-preflight";
const REVIEWER_SUB_SESSION_TASK_TYPE: &str = "reviewer_sub_session";

pub(crate) fn validate_before_session(
    args: &ReviewArgs,
    startup_env: &StartupSubtreeEnv,
) -> Result<()> {
    if args.check_verdict {
        return Ok(());
    }

    super::session_fix::validate_session_fix_before_daemon(args)?;
    super::fix_finding::validate_fix_finding_before_daemon(args)?;
    if args.fix_finding {
        return validate_fix_finding_resources_before_session(args);
    }
    super::validate_review_prompt_file(args.prompt_file.as_deref())?;
    validate_review_routing_before_session(args, startup_env)
}

fn validate_fix_finding_resources_before_session(args: &ReviewArgs) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;
    let project_config = ProjectConfig::load(&project_root)?;
    let global_config = GlobalConfig::load()?;
    let session_ref = args
        .session
        .as_deref()
        .context("--fix-finding requires --session <failed-review-session-id>")?;
    let route = super::fix_finding::load_fix_finding_route(&project_root, session_ref)?;
    validate_review_candidate_resources_before_session(
        args,
        &project_root,
        project_config.as_ref(),
        &global_config,
        route.tool,
        false,
    )
}

fn validate_review_routing_before_session(
    args: &ReviewArgs,
    startup_env: &StartupSubtreeEnv,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;
    let project_config = ProjectConfig::load(&project_root)?;
    let global_config = GlobalConfig::load()?;
    let mut effective_args = args.clone();
    let inherited_model_pin =
        crate::run_cmd_model_pin::inherited_model_pin_from_startup(startup_env);
    let inherited_trusted_pin =
        super::subtree_pin::apply_subtree_pin(&mut effective_args, inherited_model_pin);
    let (effective_tier, args_tool) =
        super::resolve::resolve_review_effective_tier(&effective_args, project_config.as_ref())?;

    crate::run_helpers::validate_tool_tier_override_flags(
        args_tool.is_some(),
        effective_tier.as_deref(),
        effective_args.force_ignore_tier_setting,
    )?;
    crate::run_helpers::validate_model_spec_tier_conflict(
        effective_args.model_spec.as_deref(),
        effective_tier.as_deref(),
        "review",
    )?;
    crate::run_helpers::enforce_tier_bypass_gate(crate::run_helpers::TierBypassGateCtx {
        project_config: project_config.as_ref(),
        global_config: &global_config,
        flags: crate::run_helpers::TierBypassGateFlags {
            model_spec: effective_args.model_spec.is_some(),
            force: false,
            force_ignore_tier_setting: effective_args.force_ignore_tier_setting,
            model: effective_args.model.is_some(),
            thinking: effective_args.thinking.is_some(),
        },
        inherited_trusted_pin,
    })?;
    super::resolve::validate_review_direct_tool_tier_restriction(
        args_tool.is_some(),
        project_config.as_ref(),
        effective_tier.as_deref(),
        effective_args.force_override_user_config,
        effective_args.force_ignore_tier_setting,
        effective_args.model_spec.is_some(),
    )?;
    let resolved_tier_name = super::resolve::resolve_review_tier_name(
        project_config.as_ref(),
        &global_config,
        effective_tier.as_deref(),
        effective_args.force_override_user_config,
        effective_args.force_ignore_tier_setting,
    )?;

    let selection =
        super::session_fix::resolve_selection_tool(&effective_args, &project_root, args_tool)?;
    let parent_tool =
        crate::run_helpers::resolve_tool(crate::run_helpers::detect_parent_tool(), &global_config);
    let resolved_selection = super::resolve::resolve_review_selection(
        selection.selection_tool,
        effective_args.model_spec.as_deref(),
        project_config.as_ref(),
        &global_config,
        parent_tool.as_deref(),
        &project_root,
        effective_args.force_override_user_config,
        effective_tier.as_deref(),
        effective_args.force_ignore_tier_setting,
        selection.direct_tool_requested,
    )?;
    let tier_active = resolved_selection.model_spec.is_some()
        && effective_args.model_spec.is_none()
        && !effective_args.force_ignore_tier_setting;
    let execution_no_failover = super::session_fix::effective_no_failover_for_session_fix(
        effective_args.no_failover,
        selection.session_fix.as_ref(),
    );
    let candidates = crate::tier_model_fallback::ordered_tier_candidates(
        resolved_selection.tool,
        resolved_selection.model_spec.as_deref(),
        resolved_tier_name.as_deref(),
        project_config.as_ref(),
        Some(&global_config),
        tier_active && !execution_no_failover,
        &resolved_selection.tier_preference_order,
    );
    for candidate_tool in
        pre_session_candidate_tools_to_validate(&candidates, selection.direct_tool_requested)
    {
        let readonly_project_root = super::resolve::resolve_review_readonly_project_root(
            effective_args.fix,
            super::resolve::resolve_review_readonly_configured(
                project_config.as_ref(),
                &global_config,
            ),
        );
        validate_review_candidate_resources_before_session(
            &effective_args,
            &project_root,
            project_config.as_ref(),
            &global_config,
            candidate_tool,
            readonly_project_root,
        )?;
    }

    Ok(())
}

fn pre_session_candidate_tools_to_validate(
    candidates: &[(ToolName, Option<String>)],
    direct_tool_requested: bool,
) -> Vec<ToolName> {
    match candidates {
        [] => Vec::new(),
        [(tool, _)] => vec![*tool],
        [(tool, _), ..] if direct_tool_requested => vec![*tool],
        [..] => Vec::new(),
    }
}

fn validate_review_candidate_resources_before_session(
    args: &ReviewArgs,
    project_root: &std::path::Path,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    tool: ToolName,
    readonly_project_root: bool,
) -> Result<()> {
    let resource_overrides = args.resource_overrides();
    let stream_mode =
        super::resolve::resolve_review_stream_mode(args.stream_stdout, args.no_stream_stdout);
    let idle_timeout_seconds = crate::pipeline::resolve_effective_idle_timeout_seconds(
        project_config,
        args.idle_timeout,
        args.timeout,
    );
    let initial_response_timeout_seconds =
        crate::pipeline::resolve_effective_initial_response_timeout_for_tool(
            project_config,
            args.initial_response_timeout,
            args.idle_timeout,
            args.timeout,
            tool.as_str(),
        );
    let liveness_dead_seconds = crate::pipeline::resolve_liveness_dead_seconds(project_config);
    let mut execution_env = global_config
        .build_execution_env(tool.as_str(), ExecutionEnvOptions::with_no_flash_fallback());
    crate::build_jobs_env::apply_build_jobs_env(&mut execution_env, args.build_jobs);
    let sandbox_input = crate::pipeline_sandbox::SandboxResolveInput {
        config: project_config,
        tool_name: tool.as_str(),
        session_id: REVIEW_PREFLIGHT_SESSION_ID,
        project_root,
        stream_mode,
        idle_timeout_seconds,
        liveness_dead_seconds,
        initial_response_timeout_seconds,
        no_fs_sandbox: args.no_fs_sandbox,
        readonly_project_root,
        extra_writable: &args.extra_writable,
        extra_readable: &args.extra_readable,
        execution_env: execution_env.as_ref(),
    };
    let execute_options = match crate::pipeline_sandbox::resolve_sandbox_options_with_overrides(
        sandbox_input,
        resource_overrides,
    ) {
        crate::pipeline_sandbox::SandboxResolution::Ok(options) => *options,
        crate::pipeline_sandbox::SandboxResolution::RequiredButUnavailable(message) => {
            anyhow::bail!(message)
        }
    };
    crate::resource_admission_soft_limit::ensure_memory_soft_limit_admission(
        Some(REVIEWER_SUB_SESSION_TASK_TYPE),
        tool.as_str(),
        execute_options
            .sandbox
            .as_ref()
            .map(|sandbox| &sandbox.isolation_plan),
    )?;
    validate_host_memory_before_session(project_root, project_config, tool, resource_overrides)
        .with_context(|| format!("review preflight for tool '{tool}'"))
}

fn validate_host_memory_before_session(
    project_root: &std::path::Path,
    project_config: Option<&ProjectConfig>,
    tool: ToolName,
    resource_overrides: RunResourceOverrides,
) -> Result<()> {
    let mut resource_guard = ResourceGuard::new(ResourceLimits {
        min_free_memory_mb: resource_overrides.resolve_min_free_memory_mb(project_config),
    });
    let projected_spawn_mb = crate::resource_admission::spawn_memory_projection_mb_with_overrides(
        project_config,
        tool.as_str(),
        resource_overrides,
    );
    let admission = crate::resource_admission::build_spawn_memory_admission(
        project_root,
        REVIEW_PREFLIGHT_SESSION_ID,
        projected_spawn_mb,
    );
    resource_guard.check_availability_with_admission(tool.as_str(), Some(admission))
}

#[cfg(test)]
#[path = "review_cmd_tests_preflight.rs"]
mod tests;
