use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TierBypassGateFlags {
    pub(crate) model_spec: bool,
    pub(crate) force: bool,
    /// Backed by `--force-ignore-tier-setting` and its `--force-tier` alias.
    pub(crate) force_ignore_tier_setting: bool,
    pub(crate) model: bool,
    pub(crate) thinking: bool,
}

pub(crate) struct TierBypassGateCtx<'a> {
    pub(crate) project_config: Option<&'a ProjectConfig>,
    pub(crate) global_config: &'a GlobalConfig,
    pub(crate) flags: TierBypassGateFlags,
    pub(crate) inherited_trusted_pin: bool,
}

pub(crate) fn enforce_tier_bypass_gate(ctx: TierBypassGateCtx<'_>) -> Result<()> {
    let Some(cfg) = ctx.project_config.filter(|cfg| !cfg.tiers.is_empty()) else {
        return Ok(());
    };

    if tier_bypass_allowed(Some(cfg), ctx.global_config, ctx.inherited_trusted_pin) {
        return Ok(());
    }

    let refused_flags = refused_tier_bypass_flags(&ctx);
    if refused_flags.is_empty() {
        return Ok(());
    }

    let mut tier_names: Vec<&str> = cfg.tiers.keys().map(String::as_str).collect();
    tier_names.sort_unstable();
    let aliases = cfg.format_tier_aliases();
    anyhow::bail!(
        "Tier bypass is disabled because [tiers] are configured. \
         Use --tier <name> to select a configured tier. \
         Available tiers: [{}]{aliases}. \
         To allow emergency exact-model/force bypasses, set \
         [tier_policy].allow_force_bypass = true in the global CSA config \
         (~/.config/cli-sub-agent/config.toml). \
         Refused flags: {}.",
        tier_names.join(", "),
        refused_flags.join(", ")
    );
}

pub(crate) fn tier_bypass_allowed(
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    inherited_trusted_pin: bool,
) -> bool {
    project_config.is_some_and(|cfg| !cfg.tiers.is_empty())
        && (global_config.tier_policy.allow_force_bypass || inherited_trusted_pin)
}

fn refused_tier_bypass_flags(ctx: &TierBypassGateCtx<'_>) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if ctx.flags.model_spec {
        flags.push("--model-spec");
    }
    if ctx.flags.force {
        flags.push("--force");
    }
    if ctx.flags.force_ignore_tier_setting {
        flags.push("--force-ignore-tier-setting");
    }
    if ctx.flags.model {
        flags.push("--model");
    }
    if ctx.flags.thinking {
        flags.push("--thinking");
    }
    flags
}
