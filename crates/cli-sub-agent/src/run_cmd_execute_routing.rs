use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolSelectionStrategy;

pub(super) fn resolve_run_no_failover(
    user_explicit_tool: bool,
    active_tier: bool,
    strategy: &ToolSelectionStrategy,
    no_failover: bool,
    allow_fallback: bool,
) -> bool {
    no_failover
        || (user_explicit_tool
            && !active_tier
            && matches!(strategy, ToolSelectionStrategy::Explicit(_))
            && !allow_fallback)
}

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
    let tier_auto_select = resolved_tier_name.is_some();
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RunSubtreePinSelection {
    pub(super) model_spec: Option<String>,
    pub(super) force_ignore_tier_setting: bool,
}

pub(super) fn resolve_run_subtree_pin_selection(
    existing_pin_active: bool,
    existing_pin_model_spec: Option<&str>,
    user_explicit_tool: bool,
    active_tier: bool,
    resolved_worker_model_spec: Option<&str>,
) -> RunSubtreePinSelection {
    if existing_pin_active {
        return RunSubtreePinSelection {
            model_spec: existing_pin_model_spec.map(str::to_string),
            force_ignore_tier_setting: true,
        };
    }

    if user_explicit_tool && active_tier {
        return RunSubtreePinSelection {
            model_spec: resolved_worker_model_spec.map(str::to_string),
            force_ignore_tier_setting: resolved_worker_model_spec.is_some(),
        };
    }

    RunSubtreePinSelection {
        model_spec: None,
        force_ignore_tier_setting: false,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct RunModelSelectionFlags {
    pub(super) tool: bool,
    pub(super) auto_route: bool,
    pub(super) skill: bool,
    pub(super) model_spec: bool,
    pub(super) model: bool,
    pub(super) thinking: bool,
    pub(super) cli_model: bool,
    pub(super) cli_thinking: bool,
    pub(super) tier: bool,
    pub(super) hint_difficulty: bool,
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
            || self.hint_difficulty
    }
}

pub(super) fn enforce_run_tier_bypass_gate(
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    selection_flags: RunModelSelectionFlags,
    force: bool,
    force_ignore_tier_setting: bool,
    inherited_trusted_pin: bool,
) -> Result<()> {
    crate::run_helpers::enforce_tier_bypass_gate(crate::run_helpers::TierBypassGateCtx {
        project_config: config,
        global_config,
        flags: crate::run_helpers::TierBypassGateFlags {
            model_spec: selection_flags.model_spec,
            force,
            force_ignore_tier_setting,
            model: selection_flags.cli_model,
            thinking: selection_flags.cli_thinking,
        },
        inherited_trusted_pin,
    })
}

pub(super) fn resolve_primary_writer_spec_for_run(
    flags: RunModelSelectionFlags,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Option<String> {
    (!flags.any_present())
        .then(|| csa_config::global::effective_primary_writer_spec(config, global_config))
        .flatten()
        .map(ToOwned::to_owned)
}

pub(super) fn resolve_run_effective_tier(
    config: Option<&ProjectConfig>,
    tier: Option<&str>,
    auto_route: Option<&str>,
    model_spec: Option<&str>,
    hint_difficulty: Option<&str>,
    frontmatter_difficulty: Option<&str>,
) -> anyhow::Result<Option<String>> {
    crate::difficulty_routing::resolve_effective_tier_with_difficulty_hint(
        config,
        tier.or(auto_route),
        model_spec,
        hint_difficulty,
        frontmatter_difficulty,
    )
}
