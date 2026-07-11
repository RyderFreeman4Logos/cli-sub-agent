use super::*;

#[tokio::test]
async fn closed_catalog_admits_batch_configured_model_during_preflight() {
    let root = tempfile::tempdir().expect("temp project");
    let _isolation = crate::test_env_lock::isolate_user_config_locked(root.path()).await;
    let config_dir = root.path().join(".csa");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "test-provider"
model = "declared"
reasoning_efforts = ["default"]
"#,
    )
    .expect("project config");
    let batch_path = root.path().join("batch.toml");
    std::fs::write(
        &batch_path,
        r#"
[[tasks]]
name = "invalid-model"
tool = "codex"
prompt = "must not dispatch"
model = "undeclared"
"#,
    )
    .expect("batch file");

    handle_batch(
        batch_path.display().to_string(),
        Some(root.path().display().to_string()),
        true,
        0,
        &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV,
    )
    .await
    .expect("active batch config model must warn instead of blocking");
}

#[tokio::test]
async fn batch_registration_accepts_future_full_model_spec() {
    let root = tempfile::tempdir().expect("temp project");
    let _isolation = crate::test_env_lock::isolate_user_config_locked(root.path()).await;
    let batch_path = root.path().join("batch.toml");
    let batch: BatchConfig = toml::from_str(
        r#"
[[tasks]]
name = "future"
tool = "codex"
prompt = "must not dispatch"
model = "codex/openai/future-batch/high"
"#,
    )
    .expect("batch config");
    let csa_config::EffectiveConfig {
        project,
        global,
        mut model_catalog,
    } = csa_config::EffectiveConfig::load(root.path()).expect("effective config");

    register_batch_model_specs(
        &mut model_catalog,
        &batch.tasks,
        &batch_path,
        project.as_ref(),
        &global,
        root.path(),
    )
    .expect("full model spec registration");

    model_catalog
        .validate_parts("codex", "openai", "future-batch", "high")
        .expect("full spec identity must match executor parsing");

    let project: csa_config::ProjectConfig = toml::from_str(
        r#"
[tiers.quality]
description = "future batch tier"
models = ["codex/openai/future-batch/high"]
"#,
    )
    .expect("tier config");
    let full_spec = "codex/openai/future-batch/high";
    project
        .enforce_tier_model_name(
            "codex",
            crate::run_helpers::model_name_for_tier_validation(Some(full_spec)),
        )
        .expect("batch full spec must survive tier policy before dispatch");
}

#[tokio::test]
async fn batch_registration_rejects_identity_whitespace_with_source_provenance() {
    let root = tempfile::tempdir().expect("temp project");
    let _isolation = crate::test_env_lock::isolate_user_config_locked(root.path()).await;
    let batch_path = root.path().join("batch.toml");

    for model in [
        "gpt-5.4 ",
        " openai/gpt-5.4",
        "openai/gpt-5.4 ",
        "openai/gpt-5.4 /high",
        "codex/openai /gpt-5.4/high",
        "codex/openai/gpt-5.4 /high",
        "codex/openai/gpt-5.4/high ",
        "codex /openai/gpt-5.4/high",
    ] {
        let batch: BatchConfig = toml::from_str(&format!(
            r#"
[[tasks]]
name = "whitespace"
tool = "codex"
prompt = "must not dispatch"
model = "{model}"
"#
        ))
        .expect("batch config");
        let csa_config::EffectiveConfig {
            project,
            global,
            mut model_catalog,
        } = csa_config::EffectiveConfig::load(root.path()).expect("effective config");

        let error = register_batch_model_specs(
            &mut model_catalog,
            &batch.tasks,
            &batch_path,
            project.as_ref(),
            &global,
            root.path(),
        )
        .expect_err("batch identity whitespace must be rejected");
        let message = format!("{error:#}");
        assert!(message.contains("whitespace"), "{model:?}: {message}");
        assert!(message.contains("tasks[0].model"), "{model:?}: {message}");
    }
}

#[tokio::test]
async fn batch_registration_splits_named_reasoning_suffix_from_model() {
    let root = tempfile::tempdir().expect("temp project");
    let _isolation = crate::test_env_lock::isolate_user_config_locked(root.path()).await;
    let batch_path = root.path().join("batch.toml");
    let batch: BatchConfig = toml::from_str(
        r#"
[[tasks]]
name = "future"
tool = "codex"
prompt = "must not dispatch"
model = "future-provider/future-model/high"
"#,
    )
    .expect("batch config");
    let csa_config::EffectiveConfig {
        project,
        global,
        mut model_catalog,
    } = csa_config::EffectiveConfig::load(root.path()).expect("effective config");

    register_batch_model_specs(
        &mut model_catalog,
        &batch.tasks,
        &batch_path,
        project.as_ref(),
        &global,
        root.path(),
    )
    .expect("batch model registration");

    model_catalog
        .validate_parts("codex", "future-provider", "future-model", "high")
        .expect("suffix reasoning must be registered against the normalized model");
}

#[tokio::test]
async fn batch_registration_uses_effective_thinking_lock_reasoning() {
    let root = tempfile::tempdir().expect("temp project");
    let _isolation = crate::test_env_lock::isolate_user_config_locked(root.path()).await;
    let config_dir = root.path().join(".csa");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "test-provider"
model = "declared"
reasoning_efforts = ["default"]

[tools.codex]
thinking_lock = "xhigh"
"#,
    )
    .expect("project config");
    let batch_path = root.path().join("batch.toml");
    let batch: BatchConfig = toml::from_str(
        r#"
[[tasks]]
name = "future"
tool = "codex"
prompt = "must not dispatch"
model = "future-provider/future-model/high"
"#,
    )
    .expect("batch config");
    let csa_config::EffectiveConfig {
        project,
        global,
        mut model_catalog,
    } = csa_config::EffectiveConfig::load(root.path()).expect("effective config");

    register_batch_model_specs(
        &mut model_catalog,
        &batch.tasks,
        &batch_path,
        project.as_ref(),
        &global,
        root.path(),
    )
    .expect("batch model registration");

    let admission = model_catalog
        .validate_parts("codex", "future-provider", "future-model", "xhigh")
        .expect("thinking-locked batch model must be admitted");
    let warning = admission
        .warning()
        .expect("future model warning")
        .to_string();
    assert!(warning.contains("tasks[0].model"), "{warning}");
    assert!(warning.contains("tools.codex.thinking_lock"), "{warning}");
}

#[tokio::test]
async fn batch_registration_resolves_alias_before_identity_admission() {
    let root = tempfile::tempdir().expect("temp project");
    let _isolation = crate::test_env_lock::isolate_user_config_locked(root.path()).await;
    let config_dir = root.path().join(".csa");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "opencode"
provider = "google"
model = "declared"
reasoning_efforts = ["high"]

[[model_catalog.entries]]
tool = "opencode"
provider = "anthropic"
model = "other"
reasoning_efforts = ["high"]

[aliases]
future = "opencode/google/gemini-2.5-pro/high"
"#,
    )
    .expect("project config");
    let batch_path = root.path().join("batch.toml");
    let batch: BatchConfig = toml::from_str(
        r#"
[[tasks]]
name = "aliased"
tool = "opencode"
prompt = "must not dispatch"
model = "future"
"#,
    )
    .expect("batch config");
    let csa_config::EffectiveConfig {
        project,
        global,
        mut model_catalog,
    } = csa_config::EffectiveConfig::load(root.path()).expect("effective config");

    register_batch_model_specs(
        &mut model_catalog,
        &batch.tasks,
        &batch_path,
        project.as_ref(),
        &global,
        root.path(),
    )
    .expect("resolved alias registration");
    model_catalog
        .validate_parts("opencode", "google", "gemini-2.5-pro", "high")
        .expect("alias target identity must be admitted");
}

#[tokio::test]
async fn batch_alias_with_nonempty_tier_uses_resolved_execution_identity() {
    let root = tempfile::tempdir().expect("temp project");
    let _isolation = crate::test_env_lock::isolate_user_config_locked(root.path()).await;
    let _tools_available = crate::test_env_lock::ScopedEnvVarRestore::set(
        crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV,
        "1",
    );
    let config_dir = root.path().join(".csa");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "gpt-future"
reasoning_efforts = ["high"]

[aliases]
future = "codex/openai/gpt-future/high"

[tiers.default]
description = "future tier"
models = ["codex/openai/gpt-future/high"]
"#,
    )
    .expect("project config");
    let batch_path = root.path().join("batch.toml");
    let batch: BatchConfig = toml::from_str(
        r#"
[[tasks]]
name = "aliased-tier"
tool = "codex"
prompt = "must not dispatch"
model = "future"
"#,
    )
    .expect("batch config");
    let csa_config::EffectiveConfig {
        project,
        global,
        mut model_catalog,
    } = csa_config::EffectiveConfig::load(root.path()).expect("effective config");

    register_batch_model_specs(
        &mut model_catalog,
        &batch.tasks,
        &batch_path,
        project.as_ref(),
        &global,
        root.path(),
    )
    .expect("resolved alias registration");
    let resolved =
        super::super::batch_catalog::resolve_batch_model(&batch.tasks[0], project.as_ref())
            .expect("batch alias must resolve");
    assert_eq!(resolved, "codex/openai/gpt-future/high");
    project
        .as_ref()
        .expect("project config")
        .enforce_tier_model_name(
            "codex",
            crate::run_helpers::model_name_for_tier_validation(Some(&resolved)),
        )
        .expect("resolved alias must satisfy the non-empty tier");

    let executor = crate::pipeline::build_and_validate_executor(
        &csa_core::types::ToolName::Codex,
        None,
        Some(&resolved),
        None,
        crate::pipeline::ConfigRefs {
            project: project.as_ref(),
            global: Some(&global),
            model_catalog: Some(&model_catalog),
        },
        false,
        false,
        false,
    )
    .await
    .expect("resolved alias must build the admitted executor");
    assert_eq!(executor.model_override(), Some("gpt-future"));
}

#[tokio::test]
async fn batch_registration_rejects_unsupported_multi_slash_identity() {
    let root = tempfile::tempdir().expect("temp project");
    let _isolation = crate::test_env_lock::isolate_user_config_locked(root.path()).await;
    let batch_path = root.path().join("batch.toml");
    let batch: BatchConfig = toml::from_str(
        r#"
[[tasks]]
name = "too-many-components"
tool = "codex"
prompt = "must not dispatch"
model = "a/b/c/d/e"
"#,
    )
    .expect("batch config");
    let csa_config::EffectiveConfig {
        project,
        global,
        mut model_catalog,
    } = csa_config::EffectiveConfig::load(root.path()).expect("effective config");

    let error = register_batch_model_specs(
        &mut model_catalog,
        &batch.tasks,
        &batch_path,
        project.as_ref(),
        &global,
        root.path(),
    )
    .expect_err("unsupported slash count must be rejected");
    let message = format!("{error:#}");
    assert!(message.contains("a/b/c/d/e"), "{message}");
    assert!(message.contains("expected model"), "{message}");
}
