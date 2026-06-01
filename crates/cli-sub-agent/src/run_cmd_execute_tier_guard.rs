use std::path::Path;

use anyhow::Result;

use csa_config::ProjectConfig;

pub(super) struct DirectToolTierGuardCtx<'a> {
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) user_explicit_tool: bool,
    pub(super) effective_tier: Option<&'a str>,
    pub(super) model_spec: Option<&'a str>,
    pub(super) force_ignore_tier_setting: bool,
    pub(super) force: bool,
    pub(super) project_root: &'a Path,
    pub(super) is_fork: bool,
    pub(super) session_arg: Option<&'a str>,
    pub(super) pre_exec_description: Option<&'a str>,
    pub(super) pre_exec_parent: Option<&'a str>,
    pub(super) explicit_tool_name: Option<&'a str>,
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
    let tier_list: Vec<&str> = cfg.tiers.keys().map(|s| s.as_str()).collect();
    let err = anyhow::anyhow!(
        "Direct --tool is blocked when tiers are configured.\n\
         Use --tier <name> for tier-based routing, --auto-route <intent> or \
         --hint-difficulty <label> to route through [tier_mapping]. \
         Emergency exact-model/force bypasses require \
         [tier_policy].allow_force_bypass = true in the global CSA config.\n\
         Example: csa run --tier <name> ...\n\
         Available tiers: {}",
        tier_list.join(", ")
    );
    Err(crate::session_guard::persist_pre_exec_error_result(
        crate::session_guard::PreExecErrorCtx {
            project_root: ctx.project_root,
            session_id: if ctx.is_fork { None } else { ctx.session_arg },
            description: ctx.pre_exec_description,
            parent: ctx.pre_exec_parent,
            tool_name: ctx.explicit_tool_name,
            task_type: Some("run"),
            tier_name: ctx.effective_tier,
            error: err,
        },
    ))
}
