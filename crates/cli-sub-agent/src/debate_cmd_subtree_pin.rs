//! SA subtree model-pin inheritance for `csa debate` (#1741).
//!
//! Extracted from `debate_cmd.rs` to keep that module under the monolith gate.

use tracing::debug;

use crate::cli::DebateArgs;

/// Apply the inherited SA subtree pin to a `csa debate` invocation in place.
///
/// Like `csa run` and `csa review`, a `csa debate` worker dispatched inside a
/// pinned SA subtree must honor the inherited model spec (`CSA_MODEL_SPEC`) when
/// the call carries no explicit `--model-spec` — so the
/// `subtree_model_pin_prompt_guard` promise holds for debate too. Precedence:
/// explicit `--model-spec` wins over the inherited env pin, which wins over
/// tier, which wins over defaults. An unpinned / depth-0 debate is unchanged
/// (tier routing preserved). Returns `true` when a parent pin was inherited.
pub(super) fn apply_subtree_pin(args: &mut DebateArgs, current_depth: u32) -> bool {
    let inherited_pin = crate::run_cmd_model_pin::apply_inherited_pin_for_review_debate(
        args.model_spec.take(),
        args.tier.take(),
        args.force_ignore_tier_setting,
        args.no_failover,
        current_depth,
    );
    args.model_spec = inherited_pin.model_spec;
    args.tier = inherited_pin.tier;
    args.force_ignore_tier_setting = inherited_pin.force_ignore_tier_setting;
    args.no_failover = inherited_pin.no_failover;
    if inherited_pin.inherited {
        debug!(
            model_spec = ?args.model_spec,
            "csa debate inheriting pinned SA subtree model spec (CSA_MODEL_SPEC); tier routing disabled"
        );
    }
    inherited_pin.inherited
}
