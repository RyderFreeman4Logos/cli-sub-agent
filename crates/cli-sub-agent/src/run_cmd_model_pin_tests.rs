use super::*;
use crate::run_cmd_tool_selection::resolve_tool_by_strategy;
use csa_config::global::DefaultsConfig;
use csa_config::{
    GlobalConfig, ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, TierStrategy, ToolConfig,
};
use csa_core::types::{ToolName, ToolSelectionStrategy};
use std::path::Path;

const PINNED_SPEC: &str = "codex/openai/gpt-5.5/xhigh";
const TEST_SESSION_ID: &str = "01KPINNEDSESSION0000000000";
const TEST_SESSION_DIR: &str = "/repo/.csa/sessions/01KPINNEDSESSION0000000000";
const TEST_PROJECT_ROOT: &str = "/repo";

#[path = "run_cmd_model_pin_sidecar_tests.rs"]
mod sidecar_tests;

fn trusted_startup_env_for_pinned_session(
    project_root: &Path,
    spec: &str,
    no_failover: bool,
) -> StartupSubtreeEnv {
    let session =
        csa_session::create_session(project_root, Some("pinned subtree"), None, Some("codex"))
            .expect("create pinned session");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    let typed_pin = resolve_subtree_model_pin(Some(spec), true, no_failover).expect("typed pin");
    sync_subtree_model_pin_sidecar(
        project_root,
        &session.meta_session_id,
        &session_dir,
        Some(&typed_pin),
    )
    .expect("write trusted pin sidecar");

    StartupSubtreeEnv::from_values(std::collections::HashMap::from([
        (
            csa_core::env::CSA_DEPTH_ENV_KEY,
            session.genealogy.depth.saturating_add(1).to_string(),
        ),
        (CSA_INTERNAL_INVOCATION_ENV_KEY, "1".to_string()),
        (CSA_SESSION_ID_ENV_KEY, session.meta_session_id),
        (CSA_SESSION_DIR_ENV_KEY, session_dir.display().to_string()),
        (CSA_PROJECT_ROOT_ENV_KEY, project_root.display().to_string()),
        (CSA_MODEL_SPEC_ENV_KEY, spec.to_string()),
        (CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1".to_string()),
        (
            CSA_NO_FAILOVER_ENV_KEY,
            if no_failover { "1" } else { "0" }.to_string(),
        ),
    ]))
}

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
fn explicit_child_same_model_spec_reuses_inherited_pin() {
    let resolution = apply_inherited_model_pin(
        RunModelPinInput {
            model_spec: Some(PINNED_SPEC.to_string()),
            tier: None,
            auto_route: None,
            force_ignore_tier_setting: true,
            no_failover: false,
        },
        Some(InheritedModelPin {
            model_spec: PINNED_SPEC.to_string(),
            force_ignore_tier_setting: true,
            no_failover: true,
        }),
    );

    assert_eq!(resolution.model_spec.as_deref(), Some(PINNED_SPEC));
    assert!(resolution.force_ignore_tier_setting);
    assert!(resolution.no_failover);
    assert!(resolution.inherited_pin.is_some());
}

#[test]
fn explicit_child_same_model_spec_with_tier_is_not_inherited() {
    let resolution = apply_inherited_model_pin(
        RunModelPinInput {
            model_spec: Some(PINNED_SPEC.to_string()),
            tier: Some("tier-4-critical".to_string()),
            auto_route: None,
            force_ignore_tier_setting: true,
            no_failover: false,
        },
        Some(InheritedModelPin {
            model_spec: PINNED_SPEC.to_string(),
            force_ignore_tier_setting: true,
            no_failover: true,
        }),
    );

    assert_eq!(resolution.model_spec.as_deref(), Some(PINNED_SPEC));
    assert_eq!(resolution.tier.as_deref(), Some("tier-4-critical"));
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
fn raw_inherited_pin_parser_requires_child_depth() {
    let lookup = |key: &str| match key {
        CSA_INTERNAL_INVOCATION_ENV_KEY => Some("1".to_string()),
        CSA_SESSION_ID_ENV_KEY => Some(TEST_SESSION_ID.to_string()),
        CSA_SESSION_DIR_ENV_KEY => Some(TEST_SESSION_DIR.to_string()),
        CSA_PROJECT_ROOT_ENV_KEY => Some(TEST_PROJECT_ROOT.to_string()),
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
fn raw_inherited_pin_parser_requires_csa_child_contract() {
    let lookup = |key: &str| match key {
        CSA_MODEL_SPEC_ENV_KEY => Some(PINNED_SPEC.to_string()),
        CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY => Some("1".to_string()),
        CSA_NO_FAILOVER_ENV_KEY => Some("1".to_string()),
        _ => None,
    };

    assert!(
        inherited_model_pin_from_lookup(1, lookup).is_none(),
        "depth plus pin env is not enough; unrelated sessions must not spoof a CSA child pin"
    );
}

/// #1741: an ambient CSA_MODEL_SPEC set in a NON-pinned root (no paired
/// CSA_FORCE_IGNORE_TIER_SETTING marker) must NOT be honored as a subtree pin —
/// the child preserves tier auto-routing.
#[test]
fn ambient_model_spec_without_force_ignore_is_not_inherited() {
    let lookup = |key: &str| match key {
        CSA_INTERNAL_INVOCATION_ENV_KEY => Some("1".to_string()),
        CSA_SESSION_ID_ENV_KEY => Some(TEST_SESSION_ID.to_string()),
        CSA_SESSION_DIR_ENV_KEY => Some(TEST_SESSION_DIR.to_string()),
        CSA_PROJECT_ROOT_ENV_KEY => Some(TEST_PROJECT_ROOT.to_string()),
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
        CSA_INTERNAL_INVOCATION_ENV_KEY => Some("1".to_string()),
        CSA_SESSION_ID_ENV_KEY => Some(TEST_SESSION_ID.to_string()),
        CSA_SESSION_DIR_ENV_KEY => Some(TEST_SESSION_DIR.to_string()),
        CSA_PROJECT_ROOT_ENV_KEY => Some(TEST_PROJECT_ROOT.to_string()),
        CSA_MODEL_SPEC_ENV_KEY => Some("not-a-valid-spec".to_string()),
        CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY => Some("1".to_string()),
        _ => None,
    };

    assert!(
        inherited_model_pin_from_lookup(1, lookup).is_none(),
        "a CSA_MODEL_SPEC that does not parse as tool/provider/model/thinking must be ignored"
    );
}

#[test]
fn subtree_prompt_guard_mentions_required_flags() {
    let guard =
        subtree_model_pin_prompt_guard(Some(PINNED_SPEC), true, true).expect("prompt guard");

    assert!(guard.contains("SHOULD omit --model-spec"));
    assert!(guard.contains("--model-spec codex/openai/gpt-5.5/xhigh"));
    assert!(guard.contains("--force-ignore-tier-setting"));
    assert!(guard.contains("--no-failover"));
    assert!(guard.contains("CSA_MODEL_SPEC"));
}

// --- `csa review` / `csa debate` subtree-pin inheritance (#1741) ---
//
// These exercise `apply_inherited_pin_for_review_debate`, the adapter both
// `handle_review` and `handle_debate` call before building executor
// candidates. Trusted inheritance uses a real session plus CSA-owned sidecar;
// raw env-only fixtures are intentionally not accepted as trusted.

#[test]
fn review_debate_inherits_env_pin_and_drops_tier() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let xdg = tempfile::tempdir().expect("xdg tempdir");
    let _xdg_guard = crate::test_env_lock::ScopedEnvVarRestore::set("XDG_STATE_HOME", xdg.path());
    let project = tempfile::tempdir().expect("project tempdir");
    let startup_env = trusted_startup_env_for_pinned_session(project.path(), PINNED_SPEC, true);

    let resolved = apply_inherited_pin_for_review_debate(
        None,
        Some("tier-4-critical".to_string()),
        false,
        false,
        inherited_model_pin_from_startup(&startup_env),
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
    let xdg = tempfile::tempdir().expect("xdg tempdir");
    let _xdg_guard = crate::test_env_lock::ScopedEnvVarRestore::set("XDG_STATE_HOME", xdg.path());
    let project = tempfile::tempdir().expect("project tempdir");
    let startup_env = trusted_startup_env_for_pinned_session(project.path(), PINNED_SPEC, true);

    let resolved = apply_inherited_pin_for_review_debate(
        None,
        Some("tier-4-critical".to_string()),
        false,
        false,
        inherited_model_pin_from_startup(&startup_env),
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
    let xdg = tempfile::tempdir().expect("xdg tempdir");
    let _xdg_guard = crate::test_env_lock::ScopedEnvVarRestore::set("XDG_STATE_HOME", xdg.path());
    let project = tempfile::tempdir().expect("project tempdir");
    let startup_env = trusted_startup_env_for_pinned_session(project.path(), PINNED_SPEC, true);
    let explicit = "gemini-cli/google/gemini-3.1-pro-preview/xhigh";

    let resolved = apply_inherited_pin_for_review_debate(
        Some(explicit.to_string()),
        None,
        false,
        false,
        inherited_model_pin_from_startup(&startup_env),
    );

    assert_eq!(resolved.model_spec.as_deref(), Some(explicit));
    assert!(!resolved.force_ignore_tier_setting);
    assert!(!resolved.no_failover);
    assert!(!resolved.inherited);
}

#[test]
fn review_debate_unpinned_preserves_tier() {
    let resolved = apply_inherited_pin_for_review_debate(
        None,
        Some("tier-4-critical".to_string()),
        false,
        false,
        None,
    );

    assert!(resolved.model_spec.is_none());
    assert_eq!(resolved.tier.as_deref(), Some("tier-4-critical"));
    assert!(!resolved.force_ignore_tier_setting);
    assert!(!resolved.no_failover);
    assert!(!resolved.inherited);
}

#[test]
fn review_debate_depth_zero_ignores_env_pin() {
    let resolved = apply_inherited_pin_for_review_debate(
        None,
        Some("tier-4-critical".to_string()),
        false,
        false,
        None,
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
    // (CSA_DEPTH>0 + child contract + trusted sidecar) and must cascade it to
    // its child unchanged.
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let xdg = tempfile::tempdir().expect("xdg tempdir");
    let _xdg_guard = crate::test_env_lock::ScopedEnvVarRestore::set("XDG_STATE_HOME", xdg.path());
    let project = tempfile::tempdir().expect("project tempdir");
    let startup_env = trusted_startup_env_for_pinned_session(project.path(), PINNED_SPEC, true);

    let inherited = inherited_model_pin_from_startup(&startup_env);
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
    assert!(
        inherited_subtree_model_pin(None).is_none(),
        "root-depth must not cascade a pin"
    );
}

#[test]
fn propagate_inherited_subtree_pin_noop_when_unpinned() {
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
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    }
}
