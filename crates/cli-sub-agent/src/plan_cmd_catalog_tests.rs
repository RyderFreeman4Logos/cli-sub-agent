use super::*;

#[test]
fn default_tier_target_preserves_catalog_governed_tier_identity() {
    let config: csa_config::ProjectConfig = toml::from_str(
        r#"
[tier_mapping]
default = "catalog-tier"

[tiers.catalog-tier]
description = "catalog tier"
models = ["codex/test-provider/declared/high"]
"#,
    )
    .expect("test config");
    let step: weave::compiler::PlanStep = toml::from_str(
        r#"
id = 1
title = "default tier"
tool = "csa"
prompt = "test"
"#,
    )
    .expect("test step");

    let target = resolve_step_tool(&step, Some(&config), None, None).expect("resolve target");
    match target {
        StepTarget::CsaTool {
            model_spec,
            tier_name,
            ..
        } => {
            assert_eq!(
                model_spec.as_deref(),
                Some("codex/test-provider/declared/high")
            );
            assert_eq!(tier_name.as_deref(), Some("catalog-tier"));
        }
        _ => panic!("expected CSA target"),
    }
}
