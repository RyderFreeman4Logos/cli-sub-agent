use crate::{EffectiveModelCatalog, GlobalConfig, ProjectConfig};
use anyhow::Result;
use std::path::Path;

/// Immutable model-sensitive configuration assembled once for a command.
#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    pub project: Option<ProjectConfig>,
    pub global: GlobalConfig,
    pub model_catalog: EffectiveModelCatalog,
}

impl EffectiveConfig {
    /// Load shipped catalog data, then global/user config, then project config.
    /// Catalog layers are parsed before tier validation or project deserialization.
    pub fn load(project_root: &Path) -> Result<Self> {
        let project_path = project_root.join(".csa").join("config.toml");
        let user_path = ProjectConfig::user_config_path();
        Self::load_with_paths(user_path.as_deref(), &project_path)
    }

    pub(crate) fn load_with_paths(user_path: Option<&Path>, project_path: &Path) -> Result<Self> {
        let mut model_catalog =
            EffectiveModelCatalog::load_with_paths(user_path, Some(project_path))?;
        let global = GlobalConfig::load_from_path(user_path)?;
        let project = ProjectConfig::load_with_paths(user_path, project_path)?;
        if let Some(config) = project.as_ref() {
            crate::configured_models::register_configured_specs(
                &mut model_catalog,
                config,
                user_path,
                project_path,
            )?;
        }
        Ok(Self {
            project,
            global,
            model_catalog,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CatalogProvenance, CatalogWarningKind};
    use std::fs;

    fn write(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn snapshot_registers_global_primary_writer_future_model_with_global_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let global_path = dir.path().join("global.toml");
        let project_path = dir.path().join("project.toml");
        write(
            &global_path,
            r#"
[preferences]
primary_writer_spec = "codex/openai/gpt-future-global/high"

[tools.codex]
thinking_lock = "xhigh"
"#,
        );
        write(&project_path, "");

        let snapshot = EffectiveConfig::load_with_paths(Some(&global_path), &project_path).unwrap();
        assert_eq!(
            snapshot.global.preferences.primary_writer_spec.as_deref(),
            Some("codex/openai/gpt-future-global/high")
        );

        let admission = snapshot
            .model_catalog
            .validate_parts("codex", "openai", "gpt-future-global", "high")
            .unwrap();
        let warning = admission.warning().expect("future model warning");
        assert_eq!(warning.kind(), CatalogWarningKind::UnverifiedModel);
        assert!(matches!(
            admission.provenance,
            CatalogProvenance::Global { ref path, ref key }
                if path == &global_path && key == "preferences.primary_writer_spec"
        ));
        let locked_admission = snapshot
            .model_catalog
            .validate_parts("codex", "openai", "gpt-future-global", "xhigh")
            .unwrap();
        let locked_warning = locked_admission
            .warning()
            .expect("locked future model warning");
        assert!(matches!(
            locked_admission.provenance,
            CatalogProvenance::Global { ref path, ref key }
                if path == &global_path && key == "preferences.primary_writer_spec"
        ));
        let rendered = locked_warning.to_string();
        assert!(
            rendered.contains("preferences.primary_writer_spec"),
            "{rendered}"
        );
        assert!(rendered.contains("tools.codex.thinking_lock"), "{rendered}");
    }

    #[test]
    fn snapshot_registers_all_project_selected_future_model_surfaces() {
        let dir = tempfile::tempdir().unwrap();
        let global_path = dir.path().join("global.toml");
        let project_path = dir.path().join("project.toml");
        write(&global_path, "");
        write(
            &project_path,
            r#"
[tools.codex]
default_model = "openai/gpt-future-default"
default_thinking = "high"

[tools.claude-code]
default_model = "anthropic/claude-future-locked"
thinking_lock = "max"

[preferences]
primary_writer_spec = "codex/openai/gpt-future-writer/medium"

[review]
tool = "codex"
model = "openai/gpt-future-review"
thinking = "xhigh"

[debate]
tool = "codex"
model = "openai/gpt-future-debate"
thinking = "low"

[aliases]
fast = "codex/openai/gpt-future-alias/high"
"#,
        );

        let snapshot = EffectiveConfig::load_with_paths(Some(&global_path), &project_path).unwrap();
        for (model, reasoning, key) in [
            ("gpt-future-default", "high", "tools.codex.default_model"),
            (
                "gpt-future-writer",
                "medium",
                "preferences.primary_writer_spec",
            ),
            ("gpt-future-review", "xhigh", "review.model"),
            ("gpt-future-debate", "low", "debate.model"),
            ("gpt-future-alias", "high", "aliases.fast"),
        ] {
            let admission = snapshot
                .model_catalog
                .validate_parts("codex", "openai", model, reasoning)
                .unwrap_or_else(|error| panic!("{model}/{reasoning}: {error}"));
            assert!(matches!(
                admission.provenance,
                CatalogProvenance::Project {
                    ref path,
                    key: ref actual_key
                } if path == &project_path && actual_key == key
            ));
        }
        let locked_admission = snapshot
            .model_catalog
            .validate_parts("claude-code", "anthropic", "claude-future-locked", "max")
            .unwrap();
        assert!(matches!(
            locked_admission.provenance,
            CatalogProvenance::Project { ref path, ref key }
                if path == &project_path && key == "tools.claude-code.default_model"
        ));
    }

    #[test]
    fn snapshot_registers_project_model_selection_surfaces() {
        let temp = tempfile::tempdir().unwrap();
        let project_path = temp.path().join("project.toml");
        fs::write(
            &project_path,
            r#"
[tools.codex]
default_model = "future-default"
default_thinking = "high"

[preferences]
primary_writer_spec = "codex/openai/future-writer/xhigh"

[review]
tool = "codex"
model = "future-review"
thinking = "medium"

[debate]
tool = "codex"
model = "future-debate"
thinking = "low"

[aliases]
fast = "codex/openai/future-alias/high"
"#,
        )
        .unwrap();

        let snapshot = EffectiveConfig::load_with_paths(None, &project_path).unwrap();
        for (model, effort) in [
            ("future-default", "high"),
            ("future-writer", "xhigh"),
            ("future-review", "medium"),
            ("future-debate", "low"),
            ("future-alias", "high"),
        ] {
            let admission = snapshot
                .model_catalog
                .validate_parts("codex", "openai", model, effort)
                .unwrap_or_else(|error| panic!("{model}/{effort} was not registered: {error}"));
            assert!(matches!(
                admission.provenance,
                CatalogProvenance::Project { .. }
            ));
        }
    }

    #[test]
    fn duplicate_model_and_reasoning_sources_are_reported_by_role() {
        let dir = tempfile::tempdir().unwrap();
        let global_path = dir.path().join("global.toml");
        let project_path = dir.path().join("project.toml");
        write(
            &global_path,
            r#"
[preferences]
primary_writer_spec = "codex/openai/gpt-future-shared/high"
"#,
        );
        write(
            &project_path,
            r#"
[tools.codex]
thinking_lock = "xhigh"

[tiers.quality]
description = "quality"
models = ["codex/openai/gpt-future-shared/high"]
"#,
        );

        let snapshot = EffectiveConfig::load_with_paths(Some(&global_path), &project_path).unwrap();
        let warning = snapshot
            .model_catalog
            .validate_parts("codex", "openai", "gpt-future-shared", "xhigh")
            .unwrap()
            .warning()
            .unwrap()
            .to_string();
        let primary = warning
            .find("preferences.primary_writer_spec")
            .expect("global selection source");
        let tier = warning
            .find("tiers.quality.models[0]")
            .expect("project selection source");
        let reasoning_marker = warning
            .find("effective reasoning selected by")
            .expect("reasoning source marker");
        let lock = warning
            .find("tools.codex.thinking_lock")
            .expect("reasoning lock source");
        assert!(primary < reasoning_marker, "{warning}");
        assert!(tier < reasoning_marker, "{warning}");
        assert!(reasoning_marker < lock, "{warning}");
        assert_eq!(warning.matches("tools.codex.thinking_lock").count(), 1);
    }

    #[test]
    fn malformed_review_model_with_extra_non_reasoning_segment_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path().join("project.toml");
        write(
            &project_path,
            r#"
[review]
tool = "codex"
model = "openai/future/extra"
thinking = "high"
"#,
        );

        let error = EffectiveConfig::load_with_paths(None, &project_path).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("review.model"), "{message}");
        assert!(message.contains("invalid value"), "{message}");
    }

    #[test]
    fn configured_review_model_with_identity_whitespace_is_rejected() {
        for model in ["openai/future-model ", "codex/openai/future-model /high"] {
            let dir = tempfile::tempdir().unwrap();
            let project_path = dir.path().join("project.toml");
            write(
                &project_path,
                &format!("[review]\ntool = \"codex\"\nmodel = \"{model}\"\nthinking = \"high\"\n"),
            );

            let error = EffectiveConfig::load_with_paths(None, &project_path).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("review.model"), "{message}");
            assert!(message.contains("whitespace"), "{message}");
        }
    }
}
