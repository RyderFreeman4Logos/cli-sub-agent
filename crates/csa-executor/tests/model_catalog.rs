use csa_core::model_catalog::EffectiveModelCatalog;
use csa_executor::{ModelSpec, model_spec::ModelSpecValidationError};

fn catalog() -> EffectiveModelCatalog {
    EffectiveModelCatalog::from_toml_str(
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "runtime-added"
reasoning_efforts = ["medium", "high"]
allow_custom_reasoning = false
"#,
        "test catalog",
    )
    .unwrap()
}

#[test]
fn model_spec_uses_injected_catalog_for_new_model() {
    let parsed = ModelSpec::parse("codex/openai/runtime-added/high").unwrap();
    parsed
        .validate_with_catalog(&catalog(), &["codex"])
        .unwrap();
}

#[test]
fn model_spec_reports_data_driven_effort_rejection() {
    let parsed = ModelSpec::parse("codex/openai/runtime-added/xhigh").unwrap();
    let error = parsed
        .validate_with_catalog(&catalog(), &["codex"])
        .unwrap_err();
    assert!(matches!(
        error,
        ModelSpecValidationError::UnsupportedReasoningEffort { .. }
    ));
}

#[test]
fn model_spec_reports_closed_catalog_unknown_model() {
    let parsed = ModelSpec::parse("codex/openai/not-declared/high").unwrap();
    let error = parsed
        .validate_with_catalog(&catalog(), &["codex"])
        .unwrap_err();
    assert!(matches!(
        error,
        ModelSpecValidationError::UnknownModel { .. }
    ));
    assert!(error.to_string().contains("test catalog"));
}
