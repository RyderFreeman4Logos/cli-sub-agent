use super::*;
use crate::test_env_lock::ScopedTestEnvVar;
use csa_config::EffectiveModelCatalog;

fn config_only_catalog() -> EffectiveModelCatalog {
    EffectiveModelCatalog::from_toml_str(
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "config-only-fake"
reasoning_efforts = ["high"]
"#,
        "tier-catalog-test",
    )
    .expect("test catalog")
}

#[test]
fn candidate_preferred_and_fallback_share_effective_catalog() {
    let _tools_available = ScopedTestEnvVar::set(TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let config = tier_tests::config_with_tier(
        "tier3",
        vec!["codex/openai/config-only-fake/high"],
        &["codex"],
    );
    let catalog = config_only_catalog();

    let candidates =
        collect_available_tier_models_with_catalog("tier3", &config, None, &catalog, &[]).unwrap();
    assert_eq!(candidates.len(), 1);

    let preferred = resolve_preferred_tool_from_tier_with_catalog(
        "tier3",
        &config,
        None,
        &catalog,
        None,
        &["codex".to_string()],
        &[],
    )
    .expect("config-only candidate should be preferred");
    assert_eq!(preferred.model_spec, "codex/openai/config-only-fake/high");

    let fallback = resolve_runtime_available_tier_fallback_with_catalog(
        &config, None, &catalog, "default", false,
    )
    .expect("catalog validation")
    .expect("config-only candidate should be eligible for fallback");
    assert_eq!(fallback.model_spec, preferred.model_spec);

    let shipped = EffectiveModelCatalog::shipped().expect("shipped catalog");
    assert!(
        collect_available_tier_models_with_catalog("tier3", &config, None, &shipped, &[],)
            .unwrap()
            .is_empty()
    );
    assert!(
        resolve_preferred_tool_from_tier_with_catalog(
            "tier3",
            &config,
            None,
            &shipped,
            None,
            &["codex".to_string()],
            &[],
        )
        .is_err()
    );
    assert!(
        resolve_runtime_available_tier_fallback_with_catalog(
            &config, None, &shipped, "default", false,
        )
        .unwrap()
        .is_none()
    );
}
