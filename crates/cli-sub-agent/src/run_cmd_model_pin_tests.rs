use super::*;
use crate::run_cmd_tool_selection::resolve_tool_by_strategy;
use csa_config::global::DefaultsConfig;
use csa_config::{
    GlobalConfig, ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, TierStrategy, ToolConfig,
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
    // Without force_ignore_tier_setting, no typed pin is produced.
    assert!(resolve_subtree_model_pin(Some(PINNED_SPEC), false, true).is_none());

    // With it, a typed pin is produced whose env entries carry the spec, the
    // paired force-ignore marker, and (here) the no-failover flag.
    let pin = resolve_subtree_model_pin(Some(PINNED_SPEC), true, true).expect("pin");
    let entries: std::collections::HashMap<&str, String> =
        pin.pin_env_entries().into_iter().collect();
    assert_eq!(
        entries.get(CSA_MODEL_SPEC_ENV_KEY).map(String::as_str),
        Some(PINNED_SPEC)
    );
    assert_eq!(
        entries
            .get(CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY)
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        entries.get(CSA_NO_FAILOVER_ENV_KEY).map(String::as_str),
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

/// #1741: an ambient CSA_MODEL_SPEC set in a NON-pinned root (no paired
/// CSA_FORCE_IGNORE_TIER_SETTING marker) must NOT be honored as a subtree pin —
/// the child preserves tier auto-routing.
#[test]
fn ambient_model_spec_without_force_ignore_is_not_inherited() {
    let lookup = |key: &str| match key {
        CSA_MODEL_SPEC_ENV_KEY => Some(PINNED_SPEC.to_string()),
        // No CSA_FORCE_IGNORE_TIER_SETTING — simulates a value leaked into the
        // shell rather than a CSA-injected pin.
        _ => None,
    };

    assert!(
        inherited_model_pin_from_lookup(2, lookup).is_none(),
        "bare CSA_MODEL_SPEC without the paired force-ignore marker must be ignored"
    );
}

/// #1741: a malformed inherited CSA_MODEL_SPEC is ignored (not applied), even
/// when the force-ignore marker is present.
#[test]
fn malformed_inherited_model_spec_is_ignored() {
    let lookup = |key: &str| match key {
        CSA_MODEL_SPEC_ENV_KEY => Some("not-a-valid-spec".to_string()),
        CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY => Some("1".to_string()),
        _ => None,
    };

    assert!(
        inherited_model_pin_from_lookup(1, lookup).is_none(),
        "a CSA_MODEL_SPEC that does not parse as tool/provider/model/thinking must be ignored"
    );
}

/// #1741: a CSA-injected pin (paired force-ignore marker + well-formed spec at
/// child depth) still propagates — the legitimate subtree-pin path stays green.
#[test]
fn csa_injected_pin_still_propagates() {
    // Build the exact child env CSA's trusted typed channel writes for a pinned
    // subtree, then prove the reader honors it (the legitimate path stays green).
    let typed_pin = resolve_subtree_model_pin(Some(PINNED_SPEC), true, true).expect("typed pin");
    let env: std::collections::HashMap<String, String> = typed_pin
        .pin_env_entries()
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

    let lookup = |key: &str| env.get(key).cloned();
    let pin = inherited_model_pin_from_lookup(1, lookup).expect("CSA-injected pin is honored");
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

// --- `csa review` / `csa debate` subtree-pin inheritance (#1741) ---
//
// These exercise `apply_inherited_pin_for_review_debate`, the adapter both
// `handle_review` and `handle_debate` call before building executor
// candidates. Env mutation is serialized via TEST_ENV_LOCK and restored by
// ScopedEnvVarRestore guards (process-wide env).

fn set_subtree_pin_env(
    spec: &str,
    force_ignore: bool,
    no_failover: bool,
) -> Vec<crate::test_env_lock::ScopedEnvVarRestore> {
    use crate::test_env_lock::ScopedEnvVarRestore;
    vec![
        ScopedEnvVarRestore::set(CSA_MODEL_SPEC_ENV_KEY, spec),
        ScopedEnvVarRestore::set(
            CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
            if force_ignore { "1" } else { "0" },
        ),
        ScopedEnvVarRestore::set(CSA_NO_FAILOVER_ENV_KEY, if no_failover { "1" } else { "0" }),
    ]
}

#[test]
fn review_debate_inherits_env_pin_and_drops_tier() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let _guards = set_subtree_pin_env(PINNED_SPEC, true, true);

    let resolved = apply_inherited_pin_for_review_debate(
        None,
        Some("tier-4-critical".to_string()),
        false,
        false,
        inherited_model_pin_from_lookup(1, |key| std::env::var(key).ok()),
    );

    assert_eq!(resolved.model_spec.as_deref(), Some(PINNED_SPEC));
    assert!(resolved.tier.is_none());
    assert!(resolved.force_ignore_tier_setting);
    assert!(resolved.no_failover);
    assert!(resolved.inherited);
}

#[test]
fn review_debate_inherited_pin_selects_pinned_model_not_tier_first_tool() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let _guards = set_subtree_pin_env(PINNED_SPEC, true, true);

    let resolved = apply_inherited_pin_for_review_debate(
        None,
        Some("tier-4-critical".to_string()),
        false,
        false,
        inherited_model_pin_from_lookup(1, |key| std::env::var(key).ok()),
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let config = config_with_tier_models(&[
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        PINNED_SPEC,
    ]);
    let global_config = GlobalConfig {
        defaults: DefaultsConfig {
            tool: Some("auto".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let selected = resolve_tool_by_strategy(
        &ToolSelectionStrategy::HeterogeneousPreferred,
        resolved.model_spec.as_deref(),
        None,
        None,
        Some(&config),
        &global_config,
        temp.path(),
        false,
        false,
        true,
        resolved.tier.as_deref(),
        resolved.force_ignore_tier_setting,
    )
    .expect("resolve inherited review/debate pin");

    assert_eq!(selected.tool, ToolName::Codex);
    assert_eq!(selected.model_spec.as_deref(), Some(PINNED_SPEC));
    assert!(selected.resolved_tier_name.is_none());
}

#[test]
fn review_debate_explicit_model_spec_overrides_env_pin() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let _guards = set_subtree_pin_env(PINNED_SPEC, true, true);
    let explicit = "gemini-cli/google/gemini-3.1-pro-preview/xhigh";

    let resolved = apply_inherited_pin_for_review_debate(
        Some(explicit.to_string()),
        None,
        false,
        false,
        inherited_model_pin_from_lookup(1, |key| std::env::var(key).ok()),
    );

    assert_eq!(resolved.model_spec.as_deref(), Some(explicit));
    assert!(!resolved.force_ignore_tier_setting);
    assert!(!resolved.no_failover);
    assert!(!resolved.inherited);
}

#[test]
fn review_debate_unpinned_preserves_tier() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    use crate::test_env_lock::ScopedEnvVarRestore;
    let _g = ScopedEnvVarRestore::unset(CSA_MODEL_SPEC_ENV_KEY);

    let resolved = apply_inherited_pin_for_review_debate(
        None,
        Some("tier-4-critical".to_string()),
        false,
        false,
        inherited_model_pin_from_lookup(1, |key| std::env::var(key).ok()),
    );

    assert!(resolved.model_spec.is_none());
    assert_eq!(resolved.tier.as_deref(), Some("tier-4-critical"));
    assert!(!resolved.force_ignore_tier_setting);
    assert!(!resolved.no_failover);
    assert!(!resolved.inherited);
}

#[test]
fn review_debate_depth_zero_ignores_env_pin() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let _guards = set_subtree_pin_env(PINNED_SPEC, true, true);

    let resolved = apply_inherited_pin_for_review_debate(
        None,
        Some("tier-4-critical".to_string()),
        false,
        false,
        inherited_model_pin_from_lookup(0, |key| std::env::var(key).ok()),
    );

    assert!(resolved.model_spec.is_none());
    assert_eq!(resolved.tier.as_deref(), Some("tier-4-critical"));
    assert!(!resolved.inherited);
}

// ── #1741 round-4/5: propagation into the spawned child env ──────────────────
//
// review/debate are pin-CONSUMING: at their spawn site they call
// resolve_subtree_model_pin with the model spec they are running as. The typed
// pin it returns is the ONLY value the executor's trusted channel writes into
// the child env, so a nested Layer-N+1 call stays pinned. (Round-5: the pin is
// carried out-of-band as a typed SubtreeModelPin, never via the env map, so a
// caller cannot spoof it by smuggling keys through request/config env.)

/// Collect a pin's env entries into a map for assertion.
fn pin_entries_map(
    pin: &csa_core::env::SubtreeModelPin,
) -> std::collections::HashMap<&str, String> {
    pin.pin_env_entries().into_iter().collect()
}

#[test]
fn review_debate_spawn_propagates_pin_into_child_env() {
    // Mirrors review_cmd_execute / debate_cmd: attempt_model_spec + the pin
    // flags (force_ignore_tier_setting from the consumed pin, no_failover).
    let pin = resolve_subtree_model_pin(Some(PINNED_SPEC), true, true)
        .expect("reviewer/debater child env must carry the subtree pin");
    let entries = pin_entries_map(&pin);
    assert_eq!(
        entries.get(CSA_MODEL_SPEC_ENV_KEY).map(String::as_str),
        Some(PINNED_SPEC)
    );
    assert_eq!(
        entries
            .get(CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY)
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        entries.get(CSA_NO_FAILOVER_ENV_KEY).map(String::as_str),
        Some("1")
    );
}

#[test]
fn review_debate_spawn_unpinned_does_not_inject_pin() {
    // Not pinned (force_ignore false) → no typed pin, so the reviewer/debater
    // child env stays clean and tier routing is preserved for nested calls.
    assert!(
        resolve_subtree_model_pin(Some(PINNED_SPEC), false, false).is_none(),
        "unpinned review/debate must not produce a pin"
    );
}

#[test]
fn propagate_inherited_subtree_pin_passes_pin_through_at_child_depth() {
    // batch / plan / claude-sub-agent path: the process inherited a pin
    // (CSA_DEPTH>0 + pin env) and must cascade it to its child unchanged.
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let mut guards = set_subtree_pin_env(PINNED_SPEC, true, true);
    guards.push(crate::test_env_lock::ScopedEnvVarRestore::set(
        "CSA_DEPTH",
        "2",
    ));

    let inherited = inherited_model_pin_from_lookup(2, |key| std::env::var(key).ok());
    let pin = inherited_subtree_model_pin(inherited.as_ref())
        .expect("inherited pin must cascade to the child env");
    let entries = pin_entries_map(&pin);
    assert_eq!(
        entries.get(CSA_MODEL_SPEC_ENV_KEY).map(String::as_str),
        Some(PINNED_SPEC)
    );
    assert_eq!(
        entries
            .get(CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY)
            .map(String::as_str),
        Some("1")
    );
}

#[test]
fn propagate_inherited_subtree_pin_noop_at_root_depth() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let mut guards = set_subtree_pin_env(PINNED_SPEC, true, true);
    // Root process (depth 0) never inherits a pin, so nothing cascades.
    guards.push(crate::test_env_lock::ScopedEnvVarRestore::set(
        "CSA_DEPTH",
        "0",
    ));

    assert!(
        inherited_subtree_model_pin(None).is_none(),
        "root-depth must not cascade a pin"
    );
}

#[test]
fn propagate_inherited_subtree_pin_noop_when_unpinned() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    use crate::test_env_lock::ScopedEnvVarRestore;
    // Child depth but no pin env → nothing to cascade.
    let _g1 = ScopedEnvVarRestore::unset(CSA_MODEL_SPEC_ENV_KEY);
    let _g2 = ScopedEnvVarRestore::set("CSA_DEPTH", "3");

    assert!(
        inherited_subtree_model_pin(None).is_none(),
        "unpinned child must not cascade a pin"
    );
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
