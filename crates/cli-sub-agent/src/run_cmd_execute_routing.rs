use csa_config::ProjectConfig;

pub(super) fn resolve_run_tier_context(
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct RunModelSelectionFlags {
    pub(super) tool: bool,
    pub(super) auto_route: bool,
    pub(super) skill: bool,
    pub(super) model_spec: bool,
    pub(super) model: bool,
    pub(super) thinking: bool,
    pub(super) tier: bool,
}

impl RunModelSelectionFlags {
    fn any_present(self) -> bool {
        self.tool
            || self.auto_route
            || self.skill
            || self.model_spec
            || self.model
            || self.thinking
            || self.tier
    }
}

pub(super) fn resolve_primary_writer_spec_for_run(
    flags: RunModelSelectionFlags,
    config: Option<&ProjectConfig>,
    global_config: &csa_config::GlobalConfig,
) -> Option<String> {
    (!flags.any_present())
        .then(|| csa_config::global::effective_primary_writer_spec(config, global_config))
        .flatten()
        .map(ToOwned::to_owned)
}
