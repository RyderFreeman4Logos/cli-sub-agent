use super::*;
use csa_executor::{Executor, ModelSpec, ThinkingBudget};

fn admitted_executor_resolved_catalog() -> csa_config::EffectiveModelCatalog {
    csa_config::EffectiveModelCatalog::from_toml_str(
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "test-provider"
model = "base"
reasoning_efforts = ["high", "xhigh"]

[[model_catalog.entries]]
tool = "codex"
provider = "test-provider"
model = "override"
reasoning_efforts = ["high", "xhigh"]
"#,
        "admitted executor resolved provenance tests",
    )
    .expect("test catalog")
}

fn admitted_executor_resolved_validation(
    executor: &Executor,
    original_model_spec: Option<&str>,
    final_model_request: Option<&str>,
) -> super::catalog_admission::ValidatedExecutorIdentity {
    validate_final_executor_identity(
        executor,
        original_model_spec,
        final_model_request,
        &admitted_executor_resolved_catalog(),
    )
    .expect("final identity should be admitted")
}

fn assert_resolved_spec(
    actual: &ModelSpec,
    tool: &str,
    provider: &str,
    model: &str,
    thinking_budget: ThinkingBudget,
) {
    assert_eq!(actual.tool, tool);
    assert_eq!(actual.provider, provider);
    assert_eq!(actual.model, model);
    assert_eq!(actual.thinking_budget, thinking_budget);
}

#[test]
fn admitted_executor_resolved_catalog_admission_returns_exact_final_identity() {
    let executor = Executor::from_tool_name(
        &ToolName::Codex,
        Some("base".to_string()),
        Some(ThinkingBudget::High),
    );

    let validated = admitted_executor_resolved_validation(
        &executor,
        Some("codex/test-provider/base/high"),
        None,
    );

    assert_resolved_spec(
        &validated.resolved_model_spec,
        "codex",
        "test-provider",
        "base",
        ThinkingBudget::High,
    );
    assert!(
        validated
            .catalog_admission
            .source_label()
            .contains("admitted executor resolved provenance tests"),
        "catalog admission must correspond to the exact validated identity"
    );
}

#[test]
fn admitted_executor_resolved_explicit_model_retains_provider_and_changes_model() {
    let executor = Executor::from_tool_name(
        &ToolName::Codex,
        Some("override".to_string()),
        Some(ThinkingBudget::High),
    );

    let validated = admitted_executor_resolved_validation(
        &executor,
        Some("codex/test-provider/base/high"),
        Some("override"),
    );

    assert_resolved_spec(
        &validated.resolved_model_spec,
        "codex",
        "test-provider",
        "override",
        ThinkingBudget::High,
    );
}

#[test]
fn admitted_executor_resolved_thinking_lock_is_in_final_snapshot() {
    let mut executor = Executor::from_tool_name(
        &ToolName::Codex,
        Some("base".to_string()),
        Some(ThinkingBudget::High),
    );
    executor.override_thinking_budget(ThinkingBudget::Xhigh);

    let validated = admitted_executor_resolved_validation(
        &executor,
        Some("codex/test-provider/base/high"),
        None,
    );

    assert_eq!(
        validated.resolved_model_spec.thinking_budget,
        ThinkingBudget::Xhigh
    );
}

#[test]
fn admitted_executor_resolved_wrapper_exposes_catalog_admitted_snapshot() {
    let executor = Executor::from_tool_name(
        &ToolName::Codex,
        Some("base".to_string()),
        Some(ThinkingBudget::High),
    );
    let validated = admitted_executor_resolved_validation(
        &executor,
        Some("codex/test-provider/base/high"),
        None,
    );
    let admitted = AdmittedExecutor::new(
        executor,
        validated.resolved_model_spec,
        validated.catalog_admission,
    );

    assert_resolved_spec(
        admitted.resolved_model_spec(),
        "codex",
        "test-provider",
        "base",
        ThinkingBudget::High,
    );
}

#[test]
fn admitted_executor_resolved_codex_fast_mode_preserves_snapshot() {
    let executor = Executor::from_tool_name(
        &ToolName::Codex,
        Some("base".to_string()),
        Some(ThinkingBudget::High),
    );
    let validated = admitted_executor_resolved_validation(
        &executor,
        Some("codex/test-provider/base/high"),
        None,
    );
    let mut admitted = AdmittedExecutor::new(
        executor,
        validated.resolved_model_spec,
        validated.catalog_admission,
    );

    admitted.enable_codex_fast_mode();

    assert!(admitted.codex_fast_mode_enabled());
    assert_resolved_spec(
        admitted.resolved_model_spec(),
        "codex",
        "test-provider",
        "base",
        ThinkingBudget::High,
    );
}
