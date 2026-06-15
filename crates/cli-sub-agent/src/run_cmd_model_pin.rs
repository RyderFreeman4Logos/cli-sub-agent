#[path = "run_cmd_model_pin_sidecar.rs"]
mod sidecar;

#[cfg(test)]
use csa_core::env::{
    CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, CSA_INTERNAL_INVOCATION_ENV_KEY, CSA_MODEL_SPEC_ENV_KEY,
    CSA_NO_FAILOVER_ENV_KEY, CSA_PROJECT_ROOT_ENV_KEY, CSA_SESSION_DIR_ENV_KEY,
    CSA_SESSION_ID_ENV_KEY,
};

pub(crate) use sidecar::sync_subtree_model_pin_sidecar;

use crate::run_cmd_tool_selection::SkillResolution;
use crate::startup_env::StartupSubtreeEnv;
use csa_core::types::ToolArg;

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
    let pin = inherited_model_pin_from_values(InheritedModelPinValues {
        current_depth: startup_env.current_depth(),
        child_contract: InheritedModelPinChildContract {
            internal_invocation: startup_env.internal_invocation(),
            session_id: startup_env.session_id(),
            session_dir: startup_env.session_dir(),
            project_root: startup_env.project_root(),
        },
        model_spec: startup_env.model_spec(),
        force_ignore_tier_setting: startup_env.force_ignore_tier_setting(),
        no_failover: startup_env.no_failover(),
    })?;

    if startup_env_trusted_pin_matches(startup_env, &pin) {
        return Some(pin);
    }

    if !sidecar::startup_env_sidecar_trusts_pin(startup_env, &pin) {
        return None;
    }

    Some(pin)
}

fn startup_env_trusted_pin_matches(
    startup_env: &StartupSubtreeEnv,
    pin: &InheritedModelPin,
) -> bool {
    startup_env.trusted_inherited_model_pin().is_some_and(
        |(model_spec, force_ignore_tier_setting, no_failover)| {
            model_spec == pin.model_spec.as_str()
                && force_ignore_tier_setting == pin.force_ignore_tier_setting
                && no_failover == pin.no_failover
        },
    )
}

#[cfg(test)]
fn inherited_model_pin_from_lookup<F>(current_depth: u32, lookup: F) -> Option<InheritedModelPin>
where
    F: Fn(&str) -> Option<String>,
{
    let raw_internal_invocation = lookup(CSA_INTERNAL_INVOCATION_ENV_KEY);
    let raw_session_id = lookup(CSA_SESSION_ID_ENV_KEY);
    let raw_session_dir = lookup(CSA_SESSION_DIR_ENV_KEY);
    let raw_project_root = lookup(CSA_PROJECT_ROOT_ENV_KEY);
    let raw_model_spec = lookup(CSA_MODEL_SPEC_ENV_KEY);
    let raw_force_ignore_tier_setting = lookup(CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY);
    let raw_no_failover = lookup(CSA_NO_FAILOVER_ENV_KEY);

    inherited_model_pin_from_values(InheritedModelPinValues {
        current_depth,
        child_contract: InheritedModelPinChildContract {
            internal_invocation: raw_internal_invocation
                .as_deref()
                .is_some_and(is_truthy_env_value),
            session_id: raw_session_id.as_deref(),
            session_dir: raw_session_dir.as_deref(),
            project_root: raw_project_root.as_deref(),
        },
        model_spec: raw_model_spec.as_deref(),
        force_ignore_tier_setting: raw_force_ignore_tier_setting
            .as_deref()
            .is_some_and(is_truthy_env_value),
        no_failover: raw_no_failover.as_deref().is_some_and(is_truthy_env_value),
    })
}

#[derive(Debug, Clone, Copy)]
struct InheritedModelPinValues<'a> {
    current_depth: u32,
    child_contract: InheritedModelPinChildContract<'a>,
    model_spec: Option<&'a str>,
    force_ignore_tier_setting: bool,
    no_failover: bool,
}

#[derive(Debug, Clone, Copy)]
struct InheritedModelPinChildContract<'a> {
    internal_invocation: bool,
    session_id: Option<&'a str>,
    session_dir: Option<&'a str>,
    project_root: Option<&'a str>,
}

impl InheritedModelPinChildContract<'_> {
    fn is_complete(self) -> bool {
        self.internal_invocation
            && self.has_session_id()
            && self.has_session_dir()
            && self.has_project_root()
    }

    fn has_session_id(self) -> bool {
        self.session_id
            .is_some_and(|value| !value.trim().is_empty())
    }

    fn has_session_dir(self) -> bool {
        self.session_dir
            .is_some_and(|value| !value.trim().is_empty())
    }

    fn has_project_root(self) -> bool {
        self.project_root
            .is_some_and(|value| !value.trim().is_empty())
    }
}

fn inherited_model_pin_from_values(
    values: InheritedModelPinValues<'_>,
) -> Option<InheritedModelPin> {
    if values.current_depth == 0 {
        return None;
    }

    let child_contract = values.child_contract;
    if !child_contract.is_complete() {
        tracing::warn!(
            current_depth = values.current_depth,
            has_session_id = child_contract.has_session_id(),
            has_session_dir = child_contract.has_session_dir(),
            has_project_root = child_contract.has_project_root(),
            internal_invocation = child_contract.internal_invocation,
            "ignoring CSA_MODEL_SPEC because the startup child contract is incomplete"
        );
        return None;
    }

    let model_spec = values.model_spec?;
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
    if !values.force_ignore_tier_setting {
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
        force_ignore_tier_setting: values.force_ignore_tier_setting,
        no_failover: values.no_failover,
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
        if input.model_spec.as_deref() == Some(pin.model_spec.as_str())
            && input.tier.is_none()
            && input.auto_route.is_none()
        {
            return RunModelPinResolution {
                model_spec: Some(pin.model_spec.clone()),
                tier: None,
                auto_route: None,
                force_ignore_tier_setting: input.force_ignore_tier_setting
                    || pin.force_ignore_tier_setting,
                no_failover: input.no_failover || pin.no_failover,
                inherited_pin: Some(pin),
            };
        }

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

pub(crate) fn explicit_tool_no_failover_from_inherited_pin(
    explicit_tool: Option<&ToolArg>,
    inherited_pin_active: bool,
    allow_fallback: bool,
) -> bool {
    inherited_pin_active
        && explicit_tool.is_some_and(|arg| matches!(arg, ToolArg::Specific(_)))
        && !allow_fallback
}

pub(crate) fn validate_inherited_model_pin_allows_explicit_tool(
    explicit_tool: Option<&ToolArg>,
    inherited_pin_active: bool,
    model_spec: Option<&str>,
) -> anyhow::Result<()> {
    if !inherited_pin_active {
        return Ok(());
    }

    let Some(ToolArg::Specific(requested_tool)) = explicit_tool else {
        return Ok(());
    };
    let Some(model_spec) = model_spec else {
        return Ok(());
    };

    let parsed = csa_executor::ModelSpec::parse(model_spec)?;
    if parsed.tool == requested_tool.as_str() {
        return Ok(());
    }

    anyhow::bail!(
        "explicit --tool {} conflicts with inherited CSA_MODEL_SPEC {model_spec} \
         (tool {}); refusing to route the explicit {} request through {}. \
         Use a matching --model-spec, clear the inherited subtree pin, or choose --tool {}.",
        requested_tool.as_str(),
        parsed.tool,
        requested_tool.as_str(),
        parsed.tool,
        parsed.tool
    )
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
/// reader, which gates on the force-ignore marker, `ModelSpec` well-formedness,
/// and the CSA-owned session sidecar).
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
         Nested CSA worker dispatches SHOULD omit --model-spec and \
         --force-ignore-tier-setting; CSA_MODEL_SPEC carries the already \
         authorized exact pin to children automatically. If a legacy workflow \
         still repeats the pin, it MUST repeat the same spec: \
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
