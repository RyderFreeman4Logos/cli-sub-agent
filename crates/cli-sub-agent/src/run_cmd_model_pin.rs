use std::collections::HashMap;

use csa_core::env::{
    CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, CSA_MODEL_SPEC_ENV_KEY, CSA_NO_FAILOVER_ENV_KEY,
};

use crate::run_cmd_tool_selection::SkillResolution;

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
    pub(crate) subtree_model_pin_active: bool,
}

pub(crate) fn inherited_model_pin_from_env(current_depth: u32) -> Option<InheritedModelPin> {
    inherited_model_pin_from_lookup(current_depth, |key| std::env::var(key).ok())
}

fn inherited_model_pin_from_lookup<F>(current_depth: u32, lookup: F) -> Option<InheritedModelPin>
where
    F: Fn(&str) -> Option<String>,
{
    if current_depth == 0 {
        return None;
    }

    let model_spec = lookup(CSA_MODEL_SPEC_ENV_KEY)?;
    let model_spec = model_spec.trim();
    if model_spec.is_empty() {
        return None;
    }

    Some(InheritedModelPin {
        model_spec: model_spec.to_string(),
        force_ignore_tier_setting: lookup(CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY)
            .as_deref()
            .is_some_and(is_truthy_env_value),
        no_failover: lookup(CSA_NO_FAILOVER_ENV_KEY)
            .as_deref()
            .is_some_and(is_truthy_env_value),
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

pub(crate) fn resolve_handle_run_model_pin(
    input: RunModelPinInput,
    current_depth: u32,
    cli_model_spec_explicit: bool,
    skill_res: &mut SkillResolution,
    user_explicit_tool: &mut bool,
) -> HandleRunModelPinResolution {
    let resolution = apply_inherited_model_pin(input, inherited_model_pin_from_env(current_depth));
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
        subtree_model_pin_active,
    }
}

pub(crate) fn inject_subtree_model_pin_env(
    extra_env: &mut Option<HashMap<String, String>>,
    model_spec: Option<&str>,
    force_ignore_tier_setting: bool,
    no_failover: bool,
) {
    let Some(model_spec) = model_spec.filter(|spec| !spec.trim().is_empty()) else {
        return;
    };
    if !force_ignore_tier_setting {
        return;
    }

    let env = extra_env.get_or_insert_with(HashMap::new);
    env.insert(CSA_MODEL_SPEC_ENV_KEY.to_string(), model_spec.to_string());
    env.insert(
        CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY.to_string(),
        "1".to_string(),
    );
    if no_failover {
        env.insert(CSA_NO_FAILOVER_ENV_KEY.to_string(), "1".to_string());
    } else {
        env.remove(CSA_NO_FAILOVER_ENV_KEY);
    }
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

fn is_truthy_env_value(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_cmd_tool_selection::resolve_tool_by_strategy;
    use csa_config::global::DefaultsConfig;
    use csa_config::{
        GlobalConfig, ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, TierStrategy,
        ToolConfig,
    };
    use csa_core::types::{ToolName, ToolSelectionStrategy};

    const PINNED_SPEC: &str = "codex/openai/gpt-5.5/xhigh";

    #[test]
    fn pinned_child_inherits_model_spec_and_drops_tier_routing() {
        let resolution = apply_inherited_model_pin(
            RunModelPinInput {
                model_spec: None,
                tier: Some("tier-4-critical".to_string()),
                auto_route: Some("complex".to_string()),
                force_ignore_tier_setting: false,
                no_failover: false,
            },
            Some(InheritedModelPin {
                model_spec: PINNED_SPEC.to_string(),
                force_ignore_tier_setting: true,
                no_failover: true,
            }),
        );

        assert_eq!(resolution.model_spec.as_deref(), Some(PINNED_SPEC));
        assert!(resolution.tier.is_none());
        assert!(resolution.auto_route.is_none());
        assert!(resolution.force_ignore_tier_setting);
        assert!(resolution.no_failover);
        assert!(resolution.inherited_pin.is_some());
    }

    #[test]
    fn inherited_pin_selects_pinned_model_instead_of_tier_first_tool() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = config_with_tier_models(&[
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
            PINNED_SPEC,
        ]);
        let resolution = apply_inherited_model_pin(
            RunModelPinInput {
                model_spec: None,
                tier: Some("tier-4-critical".to_string()),
                auto_route: None,
                force_ignore_tier_setting: false,
                no_failover: false,
            },
            Some(InheritedModelPin {
                model_spec: PINNED_SPEC.to_string(),
                force_ignore_tier_setting: true,
                no_failover: true,
            }),
        );
        let global_config = GlobalConfig {
            defaults: DefaultsConfig {
                tool: Some("auto".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let selected = resolve_tool_by_strategy(
            &ToolSelectionStrategy::HeterogeneousPreferred,
            resolution.model_spec.as_deref(),
            None,
            None,
            Some(&config),
            &global_config,
            temp.path(),
            false,
            false,
            true,
            resolution.tier.as_deref(),
            resolution.force_ignore_tier_setting,
        )
        .expect("resolve inherited pin");

        assert_eq!(selected.tool, ToolName::Codex);
        assert_eq!(selected.model_spec.as_deref(), Some(PINNED_SPEC));
        assert!(selected.resolved_tier_name.is_none());
    }

    #[test]
    fn unpinned_child_preserves_tier_routing() {
        let resolution = apply_inherited_model_pin(
            RunModelPinInput {
                model_spec: None,
                tier: Some("tier-4-critical".to_string()),
                auto_route: Some("complex".to_string()),
                force_ignore_tier_setting: false,
                no_failover: false,
            },
            None,
        );

        assert!(resolution.model_spec.is_none());
        assert_eq!(resolution.tier.as_deref(), Some("tier-4-critical"));
        assert_eq!(resolution.auto_route.as_deref(), Some("complex"));
        assert!(!resolution.force_ignore_tier_setting);
        assert!(!resolution.no_failover);
    }

    #[test]
    fn explicit_child_model_spec_overrides_inherited_pin() {
        let explicit_spec = "gemini-cli/google/gemini-3.1-pro-preview/xhigh";
        let resolution = apply_inherited_model_pin(
            RunModelPinInput {
                model_spec: Some(explicit_spec.to_string()),
                tier: None,
                auto_route: None,
                force_ignore_tier_setting: false,
                no_failover: false,
            },
            Some(InheritedModelPin {
                model_spec: PINNED_SPEC.to_string(),
                force_ignore_tier_setting: true,
                no_failover: true,
            }),
        );

        assert_eq!(resolution.model_spec.as_deref(), Some(explicit_spec));
        assert!(!resolution.force_ignore_tier_setting);
        assert!(!resolution.no_failover);
        assert!(resolution.inherited_pin.is_none());
    }

    #[test]
    fn subtree_env_requires_force_ignore_pin() {
        let mut env = None;
        inject_subtree_model_pin_env(&mut env, Some(PINNED_SPEC), false, true);
        assert!(env.is_none());

        inject_subtree_model_pin_env(&mut env, Some(PINNED_SPEC), true, true);
        let env = env.expect("pin env");
        assert_eq!(
            env.get(CSA_MODEL_SPEC_ENV_KEY).map(String::as_str),
            Some(PINNED_SPEC)
        );
        assert_eq!(
            env.get(CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY)
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            env.get(CSA_NO_FAILOVER_ENV_KEY).map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn inherited_pin_from_lookup_requires_child_depth() {
        let lookup = |key: &str| match key {
            CSA_MODEL_SPEC_ENV_KEY => Some(PINNED_SPEC.to_string()),
            CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY => Some("true".to_string()),
            CSA_NO_FAILOVER_ENV_KEY => Some("yes".to_string()),
            _ => None,
        };

        assert!(inherited_model_pin_from_lookup(0, lookup).is_none());
        let pin = inherited_model_pin_from_lookup(1, lookup).expect("child pin");
        assert_eq!(pin.model_spec, PINNED_SPEC);
        assert!(pin.force_ignore_tier_setting);
        assert!(pin.no_failover);
    }

    #[test]
    fn subtree_prompt_guard_mentions_required_flags() {
        let guard =
            subtree_model_pin_prompt_guard(Some(PINNED_SPEC), true, true).expect("prompt guard");

        assert!(guard.contains("--model-spec codex/openai/gpt-5.5/xhigh"));
        assert!(guard.contains("--force-ignore-tier-setting"));
        assert!(guard.contains("--no-failover"));
        assert!(guard.contains("CSA_MODEL_SPEC"));
    }

    fn config_with_tier_models(models: &[&str]) -> ProjectConfig {
        let mut tools = std::collections::HashMap::new();
        for tool in csa_config::global::all_known_tools() {
            let name = tool.as_str();
            tools.insert(
                name.to_string(),
                ToolConfig {
                    enabled: matches!(name, "codex" | "gemini-cli"),
                    ..Default::default()
                },
            );
        }

        ProjectConfig {
            schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: chrono::Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            acp: Default::default(),
            tools,
            review: None,
            debate: None,
            tiers: std::collections::HashMap::from([(
                "tier-4-critical".to_string(),
                TierConfig {
                    description: "Critical tier".to_string(),
                    models: models.iter().map(|model| (*model).to_string()).collect(),
                    strategy: TierStrategy::default(),
                    token_budget: None,
                    max_turns: None,
                },
            )]),
            tier_mapping: std::collections::HashMap::from([(
                "default".to_string(),
                "tier-4-critical".to_string(),
            )]),
            aliases: std::collections::HashMap::new(),
            tool_aliases: std::collections::HashMap::new(),
            preferences: None,
            github: None,
            session: Default::default(),
            memory: Default::default(),
            hooks: Default::default(),
            run: Default::default(),
            execution: Default::default(),
            session_wait: None,
            preflight: Default::default(),
            vcs: Default::default(),
            filesystem_sandbox: Default::default(),
        }
    }
}
