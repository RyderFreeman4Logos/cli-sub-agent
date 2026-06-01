use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};

pub(crate) struct TierBypassGateCtx<'a> {
    pub(crate) project_config: Option<&'a ProjectConfig>,
    pub(crate) global_config: &'a GlobalConfig,
    pub(crate) model_spec: bool,
    pub(crate) force: bool,
    pub(crate) force_ignore_tier_setting: bool,
    pub(crate) model_tier_override: bool,
    pub(crate) thinking_tier_override: bool,
    pub(crate) inherited_trusted_pin: bool,
}

pub(crate) fn enforce_tier_bypass_gate(ctx: TierBypassGateCtx<'_>) -> Result<()> {
    let Some(cfg) = ctx.project_config.filter(|cfg| !cfg.tiers.is_empty()) else {
        return Ok(());
    };

    if ctx.global_config.tier_policy.allow_force_bypass || ctx.inherited_trusted_pin {
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

fn refused_tier_bypass_flags(ctx: &TierBypassGateCtx<'_>) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if ctx.model_spec {
        flags.push("--model-spec");
    }
    if ctx.force {
        flags.push("--force");
    }
    if ctx.force_ignore_tier_setting {
        flags.push("--force-ignore-tier-setting");
    }
    if ctx.model_tier_override {
        flags.push("--model");
    }
    if ctx.thinking_tier_override {
        flags.push("--thinking");
    }
    flags
}
