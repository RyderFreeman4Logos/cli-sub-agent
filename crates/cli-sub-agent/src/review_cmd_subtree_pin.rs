//! SA subtree model-pin inheritance for `csa review` (#1741).
//!
//! Extracted from `review_cmd.rs` to keep that module under the monolith gate.

use tracing::info;

use crate::cli::ReviewArgs;

/// Apply the inherited SA subtree pin to a `csa review` invocation in place.
///
/// A pinned SA subtree (parent dispatched with `--model-spec` +
/// `--force-ignore-tier-setting --sa-mode true`) propagates the pin to nested
/// workers via `CSA_MODEL_SPEC`. `csa review` is a dispatched worker too, so it
/// must honor the inherited pin when the call carries no explicit
/// `--model-spec` — otherwise `subtree_model_pin_prompt_guard` over-promises and
/// a pinned review can silently route to the tier's first (possibly dead) tool.
/// Precedence matches `csa run`: explicit `--model-spec` wins over the inherited
/// env pin, which wins over tier, which wins over defaults. An unpinned /
/// depth-0 review is unchanged (tier routing preserved). Returns `true` when a
/// parent pin was inherited.
pub(super) fn apply_subtree_pin(
    args: &mut ReviewArgs,
    inherited_model_pin: Option<crate::run_cmd_model_pin::InheritedModelPin>,
) -> bool {
    let inherited_pin = crate::run_cmd_model_pin::apply_inherited_pin_for_review_debate(
        args.model_spec.take(),
        args.tier.take(),
        args.force_ignore_tier_setting,
        args.no_failover,
        inherited_model_pin,
    );
    args.model_spec = inherited_pin.model_spec;
    args.tier = inherited_pin.tier;
    args.force_ignore_tier_setting = inherited_pin.force_ignore_tier_setting;
    args.no_failover = inherited_pin.no_failover;
    if inherited_pin.inherited {
        info!(
            model_spec = ?args.model_spec,
            "csa review inheriting pinned SA subtree model spec (CSA_MODEL_SPEC); tier routing disabled"
        );
    }
    inherited_pin.inherited
}
