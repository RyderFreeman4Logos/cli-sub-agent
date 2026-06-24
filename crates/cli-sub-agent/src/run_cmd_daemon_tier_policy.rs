use anyhow::Result;

#[derive(Debug, Clone, Copy)]
pub(crate) struct RunDaemonTierPolicyPreflight<'a> {
    pub(crate) no_daemon: bool,
    pub(crate) daemon_child: bool,
    pub(crate) session_id: Option<&'a str>,
    pub(crate) cd: Option<&'a str>,
    pub(crate) direct_tool_requested: bool,
    pub(crate) auto_route: Option<&'a str>,
    pub(crate) hint_difficulty: Option<&'a str>,
    pub(crate) tier: Option<&'a str>,
    pub(crate) model_spec: Option<&'a str>,
    pub(crate) force: bool,
    pub(crate) force_ignore_tier_setting: bool,
}

pub(crate) fn validate_run_tier_policy_before_daemon_spawn(
    ctx: RunDaemonTierPolicyPreflight<'_>,
) -> Result<()> {
    if ctx.no_daemon
        || ctx.daemon_child
        || ctx.session_id.is_some()
        || !ctx.direct_tool_requested
        || ctx.auto_route.is_some()
        || ctx.hint_difficulty.is_some()
        || ctx.tier.is_some()
        || ctx.model_spec.is_some()
        || ctx.force
        || ctx.force_ignore_tier_setting
    {
        return Ok(());
    }

    let project_root = crate::pipeline::determine_project_root(ctx.cd)?;
    let Some(config) = csa_config::ProjectConfig::load(&project_root)? else {
        return Ok(());
    };
    if config.tiers.is_empty() {
        return Ok(());
    }

    anyhow::bail!(
        "{}",
        crate::run_helpers::format_run_direct_tool_tier_policy_error(&config)
    )
}
