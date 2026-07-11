use csa_config::{
    CatalogErrorKind, CatalogProvenance, CatalogWarningKind, EffectiveModelCatalog, ProjectConfig,
};
use std::fs;
use tempfile::tempdir;

fn write(path: &std::path::Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn fake_entry(enabled: bool, efforts: &str, custom: bool) -> String {
    format!(
        r#"
[model_catalog]
mode = "extend"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "never-before-seen-model"
enabled = {enabled}
reasoning_efforts = {efforts}
allow_custom_reasoning = {custom}
"#
    )
}

#[test]
fn shipped_catalog_preserves_existing_defaults() {
    let catalog = EffectiveModelCatalog::shipped().unwrap();
    for (tool, provider, model, effort) in [
        ("codex", "openai", "gpt-5.5", "xhigh"),
        ("claude-code", "anthropic", "default", "default"),
        ("gemini-cli", "google", "gemini-2.5-pro", "high"),
        ("opencode", "google", "gemini-2.5-pro", "high"),
    ] {
        catalog
            .validate_parts(tool, provider, model, effort)
            .unwrap_or_else(|error| panic!("shipped default rejected: {error}"));
    }
    catalog
        .validate_parts("openai-compat", "local-provider", "local-model", "medium")
        .unwrap();
}

#[test]
fn config_only_model_is_admitted_with_global_provenance() {
    let temp = tempdir().unwrap();
    let global = temp.path().join("global.toml");
    write(&global, &fake_entry(true, r#"["medium", "high"]"#, false));

    let catalog = EffectiveModelCatalog::load_with_paths(Some(&global), None).unwrap();
    let admission = catalog
        .validate_parts("codex", "openai", "never-before-seen-model", "high")
        .unwrap();
    assert!(matches!(
        admission.provenance,
        CatalogProvenance::Global { .. }
    ));
    assert!(admission.source_label().contains(global.to_str().unwrap()));
}

#[test]
fn closed_catalog_rejects_undeclared_model_with_policy_provenance() {
    let temp = tempdir().unwrap();
    let project = temp.path().join(".csa/config.toml");
    write(
        &project,
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "declared-only"
reasoning_efforts = ["high"]
"#,
    );

    let catalog = EffectiveModelCatalog::load_with_paths(None, Some(&project)).unwrap();
    let error = catalog
        .validate_parts("codex", "openai", "undeclared", "high")
        .unwrap_err();
    assert_eq!(error.kind(), CatalogErrorKind::UnknownModel);
    let rendered = error.to_string();
    assert!(rendered.contains("closed"), "{rendered}");
    assert!(rendered.contains(project.to_str().unwrap()), "{rendered}");
}

#[test]
fn active_config_spec_admits_unknown_model_with_warning_but_not_tombstone() {
    let mut catalog = EffectiveModelCatalog::from_toml_str(
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "explicitly-disabled"
enabled = false
reasoning_efforts = ["high"]
"#,
        "configured-model contract",
    )
    .unwrap();
    let configured_source = CatalogProvenance::Inline {
        source: "effective project config".to_string(),
        key: "tiers.test.models[0]".to_string(),
    };

    catalog
        .register_configured_spec(
            "codex",
            "openai",
            "backend-typo-is-still-configured",
            "high",
            configured_source.clone(),
        )
        .unwrap();
    let admission = catalog
        .validate_parts(
            "codex",
            "openai",
            "backend-typo-is-still-configured",
            "high",
        )
        .unwrap();
    let warning = admission
        .warning()
        .expect("unknown configured model warning");
    assert_eq!(warning.kind(), CatalogWarningKind::UnverifiedModel);
    assert!(warning.to_string().contains("tiers.test.models[0]"));

    catalog
        .register_configured_spec(
            "codex",
            "openai",
            "explicitly-disabled",
            "high",
            configured_source,
        )
        .unwrap();
    assert_eq!(
        catalog
            .validate_parts("codex", "openai", "explicitly-disabled", "high")
            .unwrap_err()
            .kind(),
        CatalogErrorKind::DisabledModel
    );
}

#[test]
fn configured_identity_whitespace_is_malformed_before_tombstone_lookup() {
    let catalog = EffectiveModelCatalog::from_toml_str(
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "explicitly-disabled"
enabled = false
reasoning_efforts = ["high"]
"#,
        "whitespace tombstone contract",
    )
    .unwrap();
    let source = CatalogProvenance::Inline {
        source: "batch config /tmp/batch.toml".to_string(),
        key: "tasks[0].model".to_string(),
    };

    for (tool, provider, model, reasoning) in [
        ("codex ", "openai", "explicitly-disabled", "high"),
        ("codex", " openai", "explicitly-disabled", "high"),
        ("codex", "openai", "explicitly-disabled ", "high"),
        ("codex", "openai", "explicitly-disabled", "high "),
    ] {
        let mut candidate = catalog.clone();
        let error = candidate
            .register_configured_spec(tool, provider, model, reasoning, source.clone())
            .expect_err("configured identity whitespace must fail before registration");
        let message = error.to_string();
        assert!(message.contains("whitespace"), "{message}");
        assert!(message.contains("tasks[0].model"), "{message}");
    }

    for (field, tool, provider, model, reasoning) in [
        ("tool", "codex ", "openai", "explicitly-disabled", "high"),
        (
            "provider",
            "codex",
            " openai",
            "explicitly-disabled",
            "high",
        ),
        ("model", "codex", "openai", "explicitly-disabled ", "high"),
        (
            "reasoning",
            "codex",
            "openai",
            "explicitly-disabled",
            "high ",
        ),
    ] {
        let error = catalog
            .validate_parts(tool, provider, model, reasoning)
            .expect_err("defensive admission must reject malformed identity");
        assert_eq!(error.kind(), CatalogErrorKind::MalformedIdentity);
        let message = error.to_string();
        assert!(message.contains("whitespace"), "{message}");
        assert!(message.contains(field), "{message}");
    }
}

#[test]
fn project_extend_overrides_global_and_tombstones_exact_identity() {
    let temp = tempdir().unwrap();
    let global = temp.path().join("global.toml");
    let project = temp.path().join("project.toml");
    write(&global, &fake_entry(true, r#"["medium", "high"]"#, false));
    write(&project, &fake_entry(false, r#"["high"]"#, false));

    let catalog = EffectiveModelCatalog::load_with_paths(Some(&global), Some(&project)).unwrap();
    let error = catalog
        .validate_parts("codex", "openai", "never-before-seen-model", "high")
        .unwrap_err();
    assert_eq!(error.kind(), CatalogErrorKind::DisabledModel);
    assert!(error.to_string().contains(project.to_str().unwrap()));
}

#[test]
fn replace_discards_lower_entries_but_open_scope_admits_dynamic_models() {
    let temp = tempdir().unwrap();
    let global = temp.path().join("global.toml");
    let project = temp.path().join("project.toml");
    write(&global, &fake_entry(true, r#"["high"]"#, false));
    write(
        &project,
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.open_scopes]]
tool = "openai-compat"
provider = "*"
reasoning_efforts = ["low", "medium", "high"]
allow_custom_reasoning = true
"#,
    );

    let catalog = EffectiveModelCatalog::load_with_paths(Some(&global), Some(&project)).unwrap();
    assert_eq!(
        catalog
            .validate_parts("codex", "openai", "never-before-seen-model", "high")
            .unwrap_err()
            .kind(),
        CatalogErrorKind::UnknownTool
    );
    catalog
        .validate_parts("openai-compat", "anything", "dynamic", "4096")
        .unwrap();
}

#[test]
fn effort_and_custom_reasoning_constraints_are_data_driven() {
    let temp = tempdir().unwrap();
    let global = temp.path().join("global.toml");
    write(&global, &fake_entry(true, r#"["medium", "high"]"#, false));
    let catalog = EffectiveModelCatalog::load_with_paths(Some(&global), None).unwrap();

    assert_eq!(
        catalog
            .validate_parts("codex", "openai", "never-before-seen-model", "xhigh")
            .unwrap_err()
            .kind(),
        CatalogErrorKind::UnsupportedReasoningEffort
    );
    assert_eq!(
        catalog
            .validate_parts("codex", "openai", "never-before-seen-model", "1234")
            .unwrap_err()
            .kind(),
        CatalogErrorKind::UnsupportedCustomReasoning
    );
}

#[test]
fn duplicate_and_malformed_layer_entries_are_rejected() {
    let temp = tempdir().unwrap();
    let duplicate = temp.path().join("duplicate.toml");
    write(
        &duplicate,
        &format!(
            "{}\n{}",
            fake_entry(true, r#"["high"]"#, false),
            r#"
[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "never-before-seen-model"
reasoning_efforts = ["high"]
"#
        ),
    );
    let error = EffectiveModelCatalog::load_with_paths(Some(&duplicate), None).unwrap_err();
    assert!(error.to_string().contains("duplicate"), "{error:#}");

    let malformed = temp.path().join("malformed.toml");
    write(
        &malformed,
        r#"
[model_catalog]
mode = "extend"
[[model_catalog.entries]]
tool = "codex"
provider = ""
model = "x"
reasoning_efforts = ["turbo"]
"#,
    );
    let error = EffectiveModelCatalog::load_with_paths(Some(&malformed), None).unwrap_err();
    assert!(error.to_string().contains("provider") || error.to_string().contains("turbo"));
}

#[test]
fn project_loading_does_not_prune_catalog_unknown_tier_models() {
    let temp = tempdir().unwrap();
    let project = temp.path().join(".csa/config.toml");
    write(
        &project,
        r#"
[project]
name = "catalog-prune-regression"

[tiers.test]
description = "test"
models = [
  "codex/openai/gpt-5.5/high",
  "codex/openai/never-before-seen-model/high",
]
"#,
    );

    let loaded = ProjectConfig::load_project_only(temp.path())
        .unwrap()
        .unwrap();
    assert_eq!(loaded.tiers["test"].models.len(), 2);
    assert!(loaded.tiers["test"].models[1].contains("never-before-seen-model"));
}

#[test]
fn duplicate_configured_sources_are_preserved_in_deterministic_warning_order() {
    let mut catalog = EffectiveModelCatalog::from_toml_str(
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "declared"
reasoning_efforts = ["high"]
"#,
        "duplicate provenance test",
    )
    .unwrap();
    for (source, key) in [
        ("project config", "tiers.quality.models[0]"),
        ("global config", "preferences.primary_writer_spec"),
        ("project config", "aliases.future"),
        ("project config", "tiers.quality.models[0]"),
    ] {
        catalog
            .register_configured_spec(
                "codex",
                "openai",
                "future-duplicate",
                "high",
                CatalogProvenance::Inline {
                    source: source.to_string(),
                    key: key.to_string(),
                },
            )
            .unwrap();
    }

    let render = || {
        catalog
            .validate_parts("codex", "openai", "future-duplicate", "high")
            .unwrap()
            .warning()
            .unwrap()
            .to_string()
    };
    let first = render();
    assert_eq!(first, render());
    let alias = first.find("aliases.future").expect("alias provenance");
    let primary = first
        .find("preferences.primary_writer_spec")
        .expect("primary-writer provenance");
    let tier = first
        .find("tiers.quality.models[0]")
        .expect("tier provenance");
    assert!(primary < alias && alias < tier, "{first}");
    assert_eq!(first.matches("tiers.quality.models[0]").count(), 1);
}
