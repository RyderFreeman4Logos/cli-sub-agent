//! Preflight helpers for `csa run`.

use anyhow::Result;
use std::path::Path;

use csa_config::{GlobalConfig, ProjectConfig};

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
