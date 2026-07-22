//! Preflight helpers for `csa run`.

use anyhow::Result;
use std::path::Path;

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolArg;

use crate::run_cmd_model_pin::{self, inherited_model_pin_from_startup};
use crate::run_helpers_branch_guard;
use crate::startup_env::StartupSubtreeEnv;

/// Validate `--prompt-file` with filesystem semantics before any Git pathspec
/// handling, preflight probes, or session creation (#2834).
pub(crate) fn validate_run_prompt_file(path: Option<&Path>) -> Result<()> {
    crate::run_helpers::validate_prompt_file_path(path)
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
