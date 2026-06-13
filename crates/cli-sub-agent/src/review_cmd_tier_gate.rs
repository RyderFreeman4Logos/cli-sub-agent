use std::path::Path;

use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};

use crate::cli::ReviewArgs;

pub(super) fn enforce_review_tier_bypass_gate(
    args: &ReviewArgs,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    inherited_trusted_pin: bool,
    effective_tier: Option<&str>,
    project_root: &Path,
) -> Result<()> {
    crate::run_helpers::enforce_tier_bypass_gate(crate::run_helpers::TierBypassGateCtx {
        project_config,
        global_config,
        flags: crate::run_helpers::TierBypassGateFlags {
            model_spec: args.model_spec.is_some(),
            force: false,
            force_ignore_tier_setting: args.force_ignore_tier_setting,
            model: args.model.is_some(),
            thinking: args.thinking.is_some(),
        },
        inherited_trusted_pin,
    })
    .map_err(|err| {
        super::prior_rounds::persist_tier_bypass_pre_exec_error(
            args,
            project_root,
            effective_tier,
            err,
        )
    })
}
