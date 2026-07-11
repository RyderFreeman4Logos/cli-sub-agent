use super::*;

fn closed_catalog() -> csa_config::EffectiveModelCatalog {
    csa_config::EffectiveModelCatalog::from_toml_str(
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "test-provider"
model = "declared"
reasoning_efforts = ["high"]
"#,
        "pipeline final identity test",
    )
    .expect("test catalog")
}

fn config_with_thinking_lock() -> csa_config::ProjectConfig {
    toml::from_str(
        r#"
[tools.codex]
thinking_lock = "xhigh"
"#,
    )
    .expect("test config")
}

#[tokio::test]
async fn final_boundary_rejects_undeclared_explicit_model_override() {
    let catalog = closed_catalog();
    let error = build_and_validate_executor(
        &ToolName::Codex,
        Some("codex/test-provider/declared/high"),
        Some("undeclared"),
        None,
        ConfigRefs {
            project: None,
            global: None,
            model_catalog: Some(&catalog),
        },
        false,
        false,
        false,
    )
    .await
    .expect_err("final explicit model must be catalog-admitted");

    let rendered = format!("{error:#}");
    assert!(rendered.contains("execution-boundary catalog rejection"));
    assert!(rendered.contains("undeclared"));
}

#[tokio::test]
async fn final_boundary_rejects_tombstoned_provider_qualified_override() {
    let catalog = csa_config::EffectiveModelCatalog::from_toml_str(
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "base"
reasoning_efforts = ["high"]

[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "overridden"
reasoning_efforts = ["high"]

[[model_catalog.entries]]
tool = "codex"
provider = "anthropic"
model = "overridden"
enabled = false
reasoning_efforts = ["high"]
"#,
        "provider-qualified override tombstone test",
    )
    .expect("test catalog");

    let error = build_and_validate_executor(
        &ToolName::Codex,
        Some("codex/openai/base/high"),
        Some("anthropic/overridden"),
        None,
        ConfigRefs {
            project: None,
            global: None,
            model_catalog: Some(&catalog),
        },
        false,
        false,
        false,
    )
    .await
    .expect_err("the explicit tombstoned provider must remain authoritative");

    let rendered = format!("{error:#}");
    assert!(rendered.contains("execution-boundary catalog rejection"));
    assert!(rendered.contains("anthropic"), "{rendered}");
    assert!(rendered.contains("disabled"), "{rendered}");
}

#[tokio::test]
async fn final_boundary_rejects_unsupported_explicit_thinking_override() {
    let catalog = closed_catalog();
    let error = build_and_validate_executor(
        &ToolName::Codex,
        Some("codex/test-provider/declared/high"),
        None,
        Some("xhigh"),
        ConfigRefs {
            project: None,
            global: None,
            model_catalog: Some(&catalog),
        },
        false,
        false,
        false,
    )
    .await
    .expect_err("final explicit thinking must be catalog-admitted");

    let rendered = format!("{error:#}");
    assert!(rendered.contains("unsupported reasoning effort"));
    assert!(rendered.contains("xhigh"));
}

#[tokio::test]
async fn final_boundary_admits_configured_thinking_lock_with_catalog_warning() {
    let root = tempfile::tempdir().expect("temp project");
    let _isolation = crate::test_env_lock::isolate_user_config_locked(root.path()).await;
    let _tools_available = crate::test_env_lock::ScopedEnvVarRestore::set(
        crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV,
        "1",
    );
    let mut catalog = closed_catalog();
    let config = config_with_thinking_lock();
    catalog
        .register_configured_spec(
            "codex",
            "test-provider",
            "declared",
            "xhigh",
            csa_config::CatalogProvenance::Inline {
                source: "effective project config".to_string(),
                key: "tools.codex.thinking_lock".to_string(),
            },
        )
        .expect("register configured lock");
    let executor = build_and_validate_executor(
        &ToolName::Codex,
        Some("codex/test-provider/declared/high"),
        None,
        None,
        ConfigRefs {
            project: Some(&config),
            global: None,
            model_catalog: Some(&catalog),
        },
        false,
        false,
        false,
    )
    .await
    .expect("configured thinking lock must warn instead of blocking");
    assert_eq!(
        executor.thinking_budget(),
        Some(&csa_executor::ThinkingBudget::Xhigh)
    );
    assert!(
        executor.catalog_warning_pending(),
        "construction must carry but not emit the catalog warning"
    );
    executor.emit_catalog_warning();
    assert!(
        !executor.catalog_warning_pending(),
        "the final dispatch boundary must consume the warning"
    );
    executor.emit_catalog_warning();
    assert!(
        !executor.catalog_warning_pending(),
        "repeated dispatch hooks must not emit the warning twice"
    );
}

#[tokio::test]
async fn final_boundary_rejects_unknown_model_spec_tool() {
    let catalog = closed_catalog();
    let error = build_and_validate_executor(
        &ToolName::Codex,
        Some("unknown-tool/provider/model/high"),
        None,
        None,
        ConfigRefs {
            project: None,
            global: None,
            model_catalog: Some(&catalog),
        },
        false,
        false,
        false,
    )
    .await
    .expect_err("unknown tool must remain a hard error at final admission");

    let rendered = format!("{error:#}");
    assert!(rendered.contains("tool/model-spec mismatch"), "{rendered}");
    assert!(rendered.contains("unknown-tool"), "{rendered}");
}

#[test]
fn final_boundary_rejects_tombstoned_implicit_tool_default() {
    let catalog = csa_config::EffectiveModelCatalog::from_toml_str(
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "claude-code"
provider = "anthropic"
model = "default"
enabled = false
reasoning_efforts = ["default"]
"#,
        "implicit default tombstone test",
    )
    .expect("test catalog");
    let executor = csa_executor::Executor::from_tool_name(&ToolName::ClaudeCode, None, None);
    let error = validate_final_executor_identity(&executor, None, None, &catalog)
        .expect_err("implicit tool default tombstone must remain a hard error");
    let rendered = format!("{error:#}");
    assert!(rendered.contains("execution-boundary catalog rejection"));
    assert!(
        rendered.contains("disabled by catalog tombstone"),
        "{rendered}"
    );
    assert!(
        rendered.contains("claude-code, anthropic, default"),
        "{rendered}"
    );
}
