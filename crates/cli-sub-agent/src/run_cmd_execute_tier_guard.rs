use std::path::Path;

use anyhow::Result;

use csa_config::{GlobalConfig, ProjectConfig};

use super::routing::{RunModelSelectionFlags, enforce_run_tier_bypass_gate};

#[derive(Clone, Copy)]
pub(super) struct RunPreExecErrorCtx<'a> {
    pub(super) project_root: &'a Path,
    pub(super) is_fork: bool,
    pub(super) session_arg: Option<&'a str>,
    pub(super) description: Option<&'a str>,
    pub(super) parent: Option<&'a str>,
    pub(super) tool_name: Option<&'a str>,
}

impl RunPreExecErrorCtx<'_> {
    pub(super) fn persist(self, tier_name: Option<&str>, error: anyhow::Error) -> anyhow::Error {
        crate::session_guard::persist_pre_exec_error_result(crate::session_guard::PreExecErrorCtx {
            project_root: self.project_root,
            session_id: if self.is_fork { None } else { self.session_arg },
            description: self.description,
            parent: self.parent,
            tool_name: self.tool_name,
            task_type: Some("run"),
            tier_name,
            error,
        })
    }
}

pub(super) struct DirectToolTierGuardCtx<'a> {
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) user_explicit_tool: bool,
    pub(super) effective_tier: Option<&'a str>,
    pub(super) model_spec: Option<&'a str>,
    pub(super) force_ignore_tier_setting: bool,
    pub(super) force: bool,
    pub(super) pre_exec: RunPreExecErrorCtx<'a>,
}

pub(super) fn enforce_direct_tool_tier_guard(ctx: DirectToolTierGuardCtx<'_>) -> Result<()> {
    let tiers_configured = ctx.config.is_some_and(|c| !c.tiers.is_empty());
    if !ctx.user_explicit_tool
        || !tiers_configured
        || ctx.effective_tier.is_some()
        || ctx.model_spec.is_some()
        || ctx.force_ignore_tier_setting
        || ctx.force
    {
        return Ok(());
    }

    let cfg = ctx
        .config
        .expect("tiers_configured should imply project config is present");
    let err = anyhow::anyhow!(crate::run_helpers::format_run_direct_tool_tier_policy_error(cfg));
    Err(ctx.pre_exec.persist(ctx.effective_tier, err))
}

pub(super) struct RunTierBypassPersistCtx<'a> {
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) global_config: &'a GlobalConfig,
    pub(super) selection_flags: RunModelSelectionFlags,
    pub(super) force: bool,
    pub(super) force_ignore_tier_setting: bool,
    pub(super) inherited_trusted_pin: bool,
    pub(super) pre_exec: RunPreExecErrorCtx<'a>,
    pub(super) tier_name: Option<&'a str>,
}

pub(super) fn enforce_run_tier_bypass_gate_or_persist(
    ctx: RunTierBypassPersistCtx<'_>,
) -> Result<()> {
    enforce_run_tier_bypass_gate(
        ctx.config,
        ctx.global_config,
        ctx.selection_flags,
        ctx.force,
        ctx.force_ignore_tier_setting,
        ctx.inherited_trusted_pin,
    )
    .map_err(|err| ctx.pre_exec.persist(ctx.tier_name, err))
}
