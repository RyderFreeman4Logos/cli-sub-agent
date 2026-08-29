//! Preflight helpers for `csa run`.

use anyhow::{Context, Result};
use csa_config::{ExecutionEnvOptions, GlobalConfig, ProjectConfig};
use csa_process::StreamMode;
use csa_resource::{ResourceCapability, ResourceGuard, ResourceLimits};
use std::path::{Path, PathBuf};

use csa_core::types::ToolArg;

use crate::run_cmd_model_pin::{self, inherited_model_pin_from_startup};
use crate::run_helpers_branch_guard;
use crate::startup_env::StartupSubtreeEnv;

const RUN_PREFLIGHT_SESSION_ID: &str = "run-pre-session-preflight";

/// The final resolved inputs needed to reject an unsafe writer memory envelope
/// before allocating a session directory.
pub(crate) struct RunMemorySoftLimitPreflight<'a> {
    pub project_root: &'a Path,
    pub project_config: Option<&'a ProjectConfig>,
    pub global_config: &'a GlobalConfig,
    pub tool_name: &'a str,
    pub resource_overrides: crate::run_resource_overrides::RunResourceOverrides,
    pub stream_mode: StreamMode,
    pub idle_timeout_seconds: u64,
    pub initial_response_timeout_seconds: Option<u64>,
    pub build_jobs: Option<u32>,
    pub no_fs_sandbox: bool,
    pub allow_user_daemon_ipc: bool,
    pub extra_writable: &'a [PathBuf],
    pub extra_readable: &'a [PathBuf],
}

/// Validate `--prompt-file` with filesystem semantics before any Git pathspec
/// handling, preflight probes, or session creation (#2834).
pub(crate) fn validate_run_prompt_file(path: Option<&Path>) -> Result<()> {
    crate::run_helpers::validate_prompt_file_path(path)
}

/// Resolve the writer's actual sandbox plan before creating a fresh session.
///
/// A cap that is adequate for a reviewer may still be below the writer's
/// role-specific floor; returning that error here keeps the caller from
/// receiving a new, guaranteed-to-fail session. Dynamic host-memory admission
/// waits until the attempt holds a tool slot.
pub(crate) fn validate_run_memory_soft_limit_before_session(
    input: RunMemorySoftLimitPreflight<'_>,
) -> Result<ResourceCapability> {
    let mut execution_env = input.global_config.build_execution_env(
        input.tool_name,
        ExecutionEnvOptions::with_no_flash_fallback(),
    );
    crate::build_jobs_env::apply_build_jobs_env(&mut execution_env, input.build_jobs);
    let sandbox_input = crate::pipeline_sandbox::SandboxResolveInput {
        config: input.project_config,
        tool_name: input.tool_name,
        session_id: RUN_PREFLIGHT_SESSION_ID,
        project_root: input.project_root,
        stream_mode: input.stream_mode,
        idle_timeout_seconds: input.idle_timeout_seconds,
        liveness_dead_seconds: crate::pipeline::resolve_liveness_dead_seconds(input.project_config),
        initial_response_timeout_seconds: input.initial_response_timeout_seconds,
        no_fs_sandbox: input.no_fs_sandbox,
        allow_user_daemon_ipc: input.allow_user_daemon_ipc,
        readonly_project_root: false,
        extra_writable: input.extra_writable,
        extra_readable: input.extra_readable,
        execution_env: execution_env.as_ref(),
    };
    let execute_options =
        match crate::pipeline_sandbox::resolve_pre_session_sandbox_options_with_overrides(
            sandbox_input,
            input.resource_overrides,
        ) {
            crate::pipeline_sandbox::SandboxResolution::Ok(options) => *options,
            crate::pipeline_sandbox::SandboxResolution::RequiredButUnavailable(message) => {
                anyhow::bail!(message)
            }
        };
    let resource_capability = execute_options
        .sandbox
        .as_ref()
        .map_or(ResourceCapability::None, |sandbox| {
            sandbox.isolation_plan.resource
        });
    crate::resource_admission_soft_limit::ensure_memory_soft_limit_admission(
        Some("run"),
        input.tool_name,
        execute_options
            .sandbox
            .as_ref()
            .map(|sandbox| &sandbox.isolation_plan),
    )
    .map_err(|err| {
        let err = anyhow::Error::new(err);
        let argv: Vec<String> = std::env::args().collect();
        let guidance =
            crate::no_provider_launch::soft_limit_admission_guidance_from_error_with_argv(
                input.project_root,
                None,
                input.tool_name,
                input.project_config,
                input.resource_overrides,
                &err,
                &argv,
            );
        if let Some(guidance) = guidance {
            err.context(format!(
                "writer soft-limit memory retry guidance before session creation:\n- {}",
                guidance.join("\n- ")
            ))
        } else {
            err
        }
    })
    .with_context(|| format!("run preflight for writer tool '{}'", input.tool_name))?;

    Ok(resource_capability)
}

/// Validate dynamic host memory after a writer attempt has acquired its slot.
pub(crate) fn validate_run_host_memory_after_slot_acquisition(
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
    tool_name: &str,
    resource_overrides: crate::run_resource_overrides::RunResourceOverrides,
    resource_capability: ResourceCapability,
) -> Result<()> {
    let mut resource_guard = ResourceGuard::new(ResourceLimits {
        min_free_memory_mb: resource_overrides.resolve_min_free_memory_mb(project_config),
    });
    let projected_spawn_mb = crate::resource_admission::spawn_memory_projection_mb_with_overrides(
        Some("run"),
        project_config,
        tool_name,
        resource_overrides,
        resource_capability,
    );
    let admission = crate::resource_admission::build_spawn_memory_admission(
        project_root,
        None,
        projected_spawn_mb,
    )
    .context("Failed to build host-memory admission")?;
    if !crate::resource_admission::skip_host_memory_admission_for_test() {
        resource_guard
            .check_availability_with_admission(tool_name, Some(admission))
            .map_err(|err| {
                let guidance = crate::no_provider_launch::host_memory_guidance_from_error(
                    Some("run"),
                    tool_name,
                    project_config,
                    resource_overrides,
                    &err,
                );
                if let Some(guidance) = guidance {
                    err.context(format!(
                        "writer host-memory retry guidance before session creation:\n- {}",
                        guidance.join("\n- ")
                    ))
                } else {
                    err
                }
            })
            .with_context(|| format!("run preflight for writer tool '{tool_name}'"))?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) struct EarlyPreDaemonChecks<'a> {
    pub prompt_file: Option<&'a Path>,
    pub allow_base_branch_working: bool,
    pub cd: Option<&'a str>,
    pub no_daemon: bool,
    pub daemon_child: bool,
    pub session_id: Option<&'a str>,
    pub goal_present: bool,
    pub tool: Option<&'a ToolArg>,
    pub auto_route: Option<&'a str>,
    pub hint_difficulty: Option<&'a str>,
    pub tier: Option<&'a str>,
    pub model_spec: Option<&'a str>,
    pub force: bool,
    pub force_ignore_tier_setting: bool,
    pub no_failover: bool,
    pub no_preflight: bool,
    pub is_resume: bool,
    pub startup_env: &'a StartupSubtreeEnv,
}

/// Early `csa run` checks that must run before daemon spawn / session creation.
///
/// Order is intentional:
/// 1. filesystem `--prompt-file` validation (no Git pathspec)
/// 2. protected-branch refusal
/// 3. inherited model-pin / tier policy
/// 4. AI-config symlink preflight
pub(crate) fn run_early_pre_daemon_checks(input: EarlyPreDaemonChecks<'_>) -> Result<()> {
    validate_run_prompt_file(input.prompt_file)?;

    if !input.no_daemon
        && !input.daemon_child
        && input.session_id.is_none()
        && let Some(exit_code) = run_helpers_branch_guard::evaluate_run_refusal_for_cd(
            input.allow_base_branch_working,
            input.cd,
        )?
    {
        crate::process_exit::exit_current_process(exit_code);
    }

    let effective_no_daemon = input.no_daemon || input.goal_present;
    let inherited_model_pin_resolution = run_cmd_model_pin::apply_inherited_model_pin(
        run_cmd_model_pin::RunModelPinInput {
            model_spec: input.model_spec.map(str::to_string),
            tier: input.tier.map(str::to_string),
            auto_route: input.auto_route.map(str::to_string),
            force_ignore_tier_setting: input.force_ignore_tier_setting,
            no_failover: input.no_failover,
        },
        inherited_model_pin_from_startup(input.startup_env),
    );
    let inherited_model_pin_active = inherited_model_pin_resolution.inherited_pin.is_some();
    run_cmd_model_pin::validate_inherited_model_pin_allows_explicit_tool(
        input.tool,
        inherited_model_pin_active,
        inherited_model_pin_resolution.model_spec.as_deref(),
    )?;
    crate::run_cmd_daemon::validate_run_tier_policy_before_daemon_spawn(
        crate::run_cmd_daemon::RunDaemonTierPolicyPreflight {
            no_daemon: effective_no_daemon,
            daemon_child: input.daemon_child,
            session_id: input.session_id,
            cd: input.cd,
            direct_tool_requested: input.tool.is_some(),
            auto_route: input.auto_route,
            hint_difficulty: input.hint_difficulty,
            tier: input.tier,
            model_spec: input.model_spec,
            force: input.force,
            force_ignore_tier_setting: input.force_ignore_tier_setting,
            inherited_model_pin_active,
        },
    )?;
    run_before_daemon_spawn_if_needed(
        input.cd,
        input.no_preflight,
        effective_no_daemon,
        input.daemon_child,
        input.session_id.is_some(),
        input.is_resume,
    )?;
    Ok(())
}

pub(crate) fn run_before_daemon_spawn_if_needed(
    cd: Option<&str>,
    no_preflight: bool,
    no_daemon: bool,
    daemon_child: bool,
    has_session_id: bool,
    is_resume: bool,
) -> Result<()> {
    if no_preflight || no_daemon || daemon_child || has_session_id || is_resume {
        return Ok(());
    }

    let project_root = crate::pipeline::determine_project_root(cd)?;
    let project_config = csa_config::ProjectConfig::load(&project_root)?;
    let global_config = csa_config::GlobalConfig::load()?;
    let preflight_config = project_config
        .as_ref()
        .map(|cfg| &cfg.preflight.ai_config_symlink_check)
        .unwrap_or(&global_config.preflight.ai_config_symlink_check);

    crate::preflight_symlink::run_ai_config_symlink_check(&project_root, preflight_config)
}

pub(crate) fn apply_run_preflight_override(
    project_root: &Path,
    session_arg: Option<&str>,
    no_preflight: bool,
    config: &mut Option<ProjectConfig>,
    global_config: &mut GlobalConfig,
) -> Result<()> {
    if no_preflight {
        disable_ai_config_preflight(config, global_config);
        return Ok(());
    }

    if session_arg.is_some() {
        return Ok(());
    }

    let preflight_config = config
        .as_ref()
        .map(|cfg| &cfg.preflight.ai_config_symlink_check)
        .unwrap_or(&global_config.preflight.ai_config_symlink_check);
    crate::preflight_symlink::run_ai_config_symlink_check(project_root, preflight_config)
}

fn disable_ai_config_preflight(
    config: &mut Option<ProjectConfig>,
    global_config: &mut GlobalConfig,
) {
    if let Some(project_config) = config {
        project_config.preflight.ai_config_symlink_check.enabled = false;
    } else {
        global_config.preflight.ai_config_symlink_check.enabled = false;
    }
}

#[cfg(test)]
#[path = "run_cmd_preflight_tests.rs"]
mod tests;
