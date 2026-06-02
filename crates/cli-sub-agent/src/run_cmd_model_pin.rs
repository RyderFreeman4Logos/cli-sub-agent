#[cfg(test)]
use csa_core::env::{
    CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, CSA_MODEL_SPEC_ENV_KEY, CSA_NO_FAILOVER_ENV_KEY,
};

use crate::run_cmd_tool_selection::SkillResolution;
use crate::startup_env::StartupSubtreeEnv;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InheritedModelPin {
    pub(crate) model_spec: String,
    pub(crate) force_ignore_tier_setting: bool,
    pub(crate) no_failover: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunModelPinInput {
    pub(crate) model_spec: Option<String>,
    pub(crate) tier: Option<String>,
    pub(crate) auto_route: Option<String>,
    pub(crate) force_ignore_tier_setting: bool,
    pub(crate) no_failover: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunModelPinResolution {
    pub(crate) model_spec: Option<String>,
    pub(crate) tier: Option<String>,
    pub(crate) auto_route: Option<String>,
    pub(crate) force_ignore_tier_setting: bool,
    pub(crate) no_failover: bool,
    pub(crate) inherited_pin: Option<InheritedModelPin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HandleRunModelPinResolution {
    pub(crate) model_spec: Option<String>,
    pub(crate) tier: Option<String>,
    pub(crate) auto_route: Option<String>,
    pub(crate) force_ignore_tier_setting: bool,
    pub(crate) no_failover: bool,
    pub(crate) inherited_trusted_pin: bool,
    pub(crate) subtree_model_pin_active: bool,
}

pub(crate) fn inherited_model_pin_from_startup(
    startup_env: &StartupSubtreeEnv,
) -> Option<InheritedModelPin> {
    inherited_model_pin_from_values(
        startup_env.current_depth(),
        startup_env.model_spec(),
        startup_env.force_ignore_tier_setting(),
        startup_env.no_failover(),
    )
}

#[cfg(test)]
fn inherited_model_pin_from_lookup<F>(current_depth: u32, lookup: F) -> Option<InheritedModelPin>
where
    F: Fn(&str) -> Option<String>,
{
    inherited_model_pin_from_values(
        current_depth,
        lookup(CSA_MODEL_SPEC_ENV_KEY).as_deref(),
        lookup(CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY)
            .as_deref()
            .is_some_and(is_truthy_env_value),
        lookup(CSA_NO_FAILOVER_ENV_KEY)
            .as_deref()
            .is_some_and(is_truthy_env_value),
    )
}

fn inherited_model_pin_from_values(
    current_depth: u32,
    model_spec: Option<&str>,
    force_ignore_tier_setting: bool,
    no_failover: bool,
) -> Option<InheritedModelPin> {
    if current_depth == 0 {
        return None;
    }

    let model_spec = model_spec?;
    let model_spec = model_spec.trim();
    if model_spec.is_empty() {
        return None;
    }

    // Defense-in-depth (#1741): a CSA-injected subtree pin is ALWAYS written
    // together with CSA_FORCE_IGNORE_TIER_SETTING (see
    // `SubtreeModelPin::pin_env_entries`, applied by the executor's trusted
    // typed channel). A bare CSA_MODEL_SPEC without the paired marker therefore
    // cannot be a CSA pin — ignore it so a stray/ambient value never silently
    // pins the subtree and drops tier routing. (The ambient value is also
    // reserved at the spawn boundary; this is the reader-side belt to the
    // spawn-side braces.)
    if !force_ignore_tier_setting {
        tracing::warn!(
            model_spec,
            "ignoring CSA_MODEL_SPEC without paired CSA_FORCE_IGNORE_TIER_SETTING \
             (not a CSA-injected subtree pin)"
        );
        return None;
    }

    // Validate the inherited spec is well-formed (tool/provider/model/thinking)
    // before applying. A malformed/garbage value is ignored rather than pinned.
    if let Err(err) = csa_executor::ModelSpec::parse(model_spec) {
        tracing::warn!(
            model_spec,
            error = %err,
            "ignoring malformed inherited CSA_MODEL_SPEC subtree pin"
        );
        return None;
    }

    Some(InheritedModelPin {
        model_spec: model_spec.to_string(),
        force_ignore_tier_setting,
        no_failover,
    })
}

pub(crate) fn apply_inherited_model_pin(
    input: RunModelPinInput,
    inherited_pin: Option<InheritedModelPin>,
) -> RunModelPinResolution {
    let Some(pin) = inherited_pin else {
        return RunModelPinResolution {
            model_spec: input.model_spec,
            tier: input.tier,
            auto_route: input.auto_route,
            force_ignore_tier_setting: input.force_ignore_tier_setting,
            no_failover: input.no_failover,
            inherited_pin: None,
        };
    };

    if input.model_spec.is_some() {
        return RunModelPinResolution {
            model_spec: input.model_spec,
            tier: input.tier,
            auto_route: input.auto_route,
            force_ignore_tier_setting: input.force_ignore_tier_setting,
            no_failover: input.no_failover,
            inherited_pin: None,
        };
    }

    RunModelPinResolution {
        model_spec: Some(pin.model_spec.clone()),
        tier: None,
        auto_route: None,
        force_ignore_tier_setting: input.force_ignore_tier_setting || pin.force_ignore_tier_setting,
        no_failover: input.no_failover || pin.no_failover,
        inherited_pin: Some(pin),
    }
}

/// Resolved subtree pin for the `csa review` / `csa debate` execution paths.
///
/// Mirrors the `csa run` inheritance: when the command carries no explicit
/// `--model-spec` but a parent pinned the SA subtree via `CSA_MODEL_SPEC`
/// (at child depth > 0), the child inherits the spec (and the OR-ed
/// `force_ignore_tier_setting` / `no_failover`) and drops tier routing so the
/// pinned tool is selected instead of the tier's first tool. An explicit
/// `--model-spec` on the call overrides; an unpinned / depth-0 invocation is
/// returned unchanged so tier routing is preserved (#1741).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InheritedPinForReviewDebate {
    pub(crate) model_spec: Option<String>,
    pub(crate) tier: Option<String>,
    pub(crate) force_ignore_tier_setting: bool,
    pub(crate) no_failover: bool,
    /// True when a parent subtree pin was actually inherited (i.e. the spec/tier
    /// were overridden from the environment). Unchanged on explicit-spec,
    /// unpinned, or depth-0 paths.
    pub(crate) inherited: bool,
}

/// Apply the inherited SA subtree pin to a `csa review` / `csa debate` call.
///
/// Reuses the same startup-captured inherited pin +
/// [`apply_inherited_model_pin`] machinery as `csa run`, so precedence is
/// identical: explicit `--model-spec` wins over the inherited pin, which wins
/// over tier, which wins over defaults. `auto_route` has no analog for
/// review/debate, so `None` is passed through.
pub(crate) fn apply_inherited_pin_for_review_debate(
    model_spec: Option<String>,
    tier: Option<String>,
    force_ignore_tier_setting: bool,
    no_failover: bool,
    inherited_pin: Option<InheritedModelPin>,
) -> InheritedPinForReviewDebate {
    let resolution = apply_inherited_model_pin(
        RunModelPinInput {
            model_spec,
            tier,
            auto_route: None,
            force_ignore_tier_setting,
            no_failover,
        },
        inherited_pin,
    );
    InheritedPinForReviewDebate {
        model_spec: resolution.model_spec,
        tier: resolution.tier,
        force_ignore_tier_setting: resolution.force_ignore_tier_setting,
        no_failover: resolution.no_failover,
        inherited: resolution.inherited_pin.is_some(),
    }
}

pub(crate) fn resolve_handle_run_model_pin(
    input: RunModelPinInput,
    inherited_pin: Option<InheritedModelPin>,
    cli_model_spec_explicit: bool,
    skill_res: &mut SkillResolution,
    user_explicit_tool: &mut bool,
) -> HandleRunModelPinResolution {
    let resolution = apply_inherited_model_pin(input, inherited_pin);
    let inherited_pin_active = resolution.inherited_pin.is_some();
    if inherited_pin_active {
        skill_res.tool = None;
        skill_res.model = None;
        skill_res.thinking = None;
        *user_explicit_tool = false;
    }
    let subtree_model_pin_active =
        resolution.force_ignore_tier_setting && (cli_model_spec_explicit || inherited_pin_active);

    HandleRunModelPinResolution {
        model_spec: resolution.model_spec,
        tier: resolution.tier,
        auto_route: resolution.auto_route,
        force_ignore_tier_setting: resolution.force_ignore_tier_setting,
        no_failover: resolution.no_failover,
        inherited_trusted_pin: inherited_pin_active,
        subtree_model_pin_active,
    }
}

/// Resolve CSA's authoritative subtree model pin for a spawn (#1741).
///
/// Returns a typed [`SubtreeModelPin`] ONLY when CSA itself decided to pin:
/// a non-blank `model_spec` together with `force_ignore_tier_setting` (a CSA
/// subtree pin is, by definition, a force-ignore-tier pin). Otherwise returns
/// `None`.
///
/// The returned pin is carried OUT-OF-BAND from the generic `extra_env` map —
/// it is NEVER written into `extra_env` — and is applied to the child by the
/// executor's trusted typed channel, after every generic env merge (which
/// unconditionally strips the pin keys). This makes pin spoofing impossible by
/// construction: no user/request/config env can introduce the pin keys.
///
/// `model_spec` MUST originate from validated CSA state (the spec the caller
/// resolved itself, or one returned by the startup-captured inherited-pin
/// reader, which gates on the force-ignore marker + `ModelSpec` well-formedness).
pub(crate) fn resolve_subtree_model_pin(
    model_spec: Option<&str>,
    force_ignore_tier_setting: bool,
    no_failover: bool,
) -> Option<csa_core::env::SubtreeModelPin> {
    let model_spec = model_spec.filter(|spec| !spec.trim().is_empty())?;
    if !force_ignore_tier_setting {
        return None;
    }
    csa_core::env::SubtreeModelPin::from_validated_spec(model_spec, no_failover)
}

/// Resolve an inherited subtree model pin to cascade to a child CSA-recursion
/// spawn that did NOT itself consume the pin for tool selection.
///
/// Used by CSA-recursion spawn sites that pick their own per-spawn tool/model
/// from explicit input (batch task, plan step, claude-sub-agent): they still
/// must cascade an inherited pin so a nested Layer-N+1 call stays pinned all the
/// way down (#1741). The returned [`SubtreeModelPin`] is carried out-of-band
/// from `extra_env` and applied by the executor's trusted typed channel.
///
/// Pin-CONSUMING sites (csa run / review / debate) instead call
/// [`resolve_subtree_model_pin`] with the spec they resolved. This function is
/// the no-consume counterpart. Returns `None` when the parent did not pin
/// (depth 0 or no pin env).
pub(crate) fn inherited_subtree_model_pin(
    inherited: Option<&InheritedModelPin>,
) -> Option<csa_core::env::SubtreeModelPin> {
    let inherited = inherited?;
    resolve_subtree_model_pin(
        Some(&inherited.model_spec),
        inherited.force_ignore_tier_setting,
        inherited.no_failover,
    )
}

pub(crate) fn subtree_model_pin_prompt_guard(
    model_spec: Option<&str>,
    force_ignore_tier_setting: bool,
    no_failover: bool,
) -> Option<String> {
    let model_spec = model_spec.filter(|spec| !spec.trim().is_empty())?;
    if !force_ignore_tier_setting {
        return None;
    }

    let no_failover_flag = if no_failover { " --no-failover" } else { "" };
    Some(format!(
        "<csa-subtree-model-pin>\n\
         The caller pinned this CSA subtree to --model-spec {model_spec} \
         with --force-ignore-tier-setting.\n\
         Every nested CSA worker dispatch you create MUST reuse: \
         --model-spec {model_spec} --force-ignore-tier-setting{no_failover_flag}\n\
         Do not replace this pin with --tier or --auto-route unless the user \
         explicitly changes the pin.\n\
         Child csa invocations that omit --model-spec inherit CSA_MODEL_SPEC \
         automatically.\n\
         </csa-subtree-model-pin>"
    ))
}

#[cfg(test)]
fn is_truthy_env_value(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
#[path = "run_cmd_model_pin_tests.rs"]
mod tests;
