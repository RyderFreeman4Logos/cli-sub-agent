use crate::{
    EffectiveModelCatalog, GlobalConfig, ProjectConfig, ProjectConvergenceCompletionPolicy,
    parse_project_convergence_completion_policy,
};
use anyhow::{Context, Result};
use std::path::Path;

/// Immutable model-sensitive configuration assembled once for a command.
#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    pub project: Option<ProjectConfig>,
    pub global: GlobalConfig,
    /// Raw project-only restrictions kept separate from the normal merged project view.
    pub project_convergence_completion: Option<ProjectConvergenceCompletionPolicy>,
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
        Self::load_with_paths_and_reader(user_path, project_path, read_optional_source)
    }

    fn load_with_paths_and_reader<F>(
        user_path: Option<&Path>,
        project_path: &Path,
        mut read_source: F,
    ) -> Result<Self>
    where
        F: FnMut(&Path) -> Result<Option<String>>,
    {
        let user_content = user_path.map(&mut read_source).transpose()?.flatten();
        let project_content = read_source(project_path)?;
        let mut model_catalog = EffectiveModelCatalog::load_from_captured_sources(
            user_path.zip(user_content.as_deref()),
            project_content
                .as_deref()
                .map(|contents| (project_path, contents)),
        )?;
        let global = GlobalConfig::load_from_captured_source(user_path, user_content.as_deref())?;
        let project = ProjectConfig::load_from_captured_sources(
            user_path,
            user_content.as_deref(),
            project_path,
            project_content.as_deref(),
        )?;
        let project_raw = project_content
            .as_deref()
            .and_then(|contents| toml::from_str(contents).ok());
        let project_convergence_completion = project_raw
            .as_ref()
            .map(parse_project_convergence_completion_policy)
            .transpose()
            .context("Invalid project convergence completion policy")?
            .flatten();
        if let Some(config) = project.as_ref() {
            crate::configured_models::register_configured_specs(
                &mut model_catalog,
                config,
                user_path,
                project_path,
                project_raw,
            )?;
        }
        Ok(Self {
            project,
            global,
            project_convergence_completion,
            model_catalog,
        })
    }
}

fn read_optional_source(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("Failed to read config source: {}", path.display()))
        }
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
    fn snapshot_keeps_project_completion_restrictions_below_global_safety_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let global_path = dir.path().join("global.toml");
        let project_path = dir.path().join("project.toml");
        write(
            &global_path,
            r#"
[convergence_completion]
allow_execution = true
allow_provider_egress = true
allow_shell_commands = true
allow_credential_inheritance = true
max_retention_days = 30
"#,
        );
        write(
            &project_path,
            r#"
[convergence_completion]
allow_provider_egress = false
max_retention_days = 7
"#,
        );

        let snapshot = EffectiveConfig::load_with_paths(Some(&global_path), &project_path)
            .expect("project policy should be allowed to tighten the global ceiling");
        let effective = crate::ConvergenceCompletionPolicy::effective(
            &snapshot.global.convergence_completion,
            snapshot.project_convergence_completion.as_ref(),
        );
        let error = effective
            .require_explicit_execution(true)
            .expect_err("project egress restriction must remain effective");
        assert!(error.to_string().contains("allow_provider_egress"));
        assert_eq!(effective.max_retention_days(), 7);
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

    #[test]
    fn snapshot_reads_each_source_once_and_uses_captured_bytes_for_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let global_path = dir.path().join("global.toml");
        let project_path = dir.path().join("project.toml");
        let global_generation = r#"
[preferences]
primary_writer_spec = "codex/openai/gpt-future-snapshot/high"
"#;
        let captured_project_generation = "";
        let later_project_generation = r#"
[preferences]
primary_writer_spec = "codex/openai/gpt-future-snapshot/high"

[model_catalog]
mode = "extend"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "gpt-future-snapshot"
enabled = false
reasoning_efforts = ["high"]
"#;
        write(&project_path, later_project_generation);

        let global_reads = std::cell::Cell::new(0);
        let project_reads = std::cell::Cell::new(0);
        let snapshot = EffectiveConfig::load_with_paths_and_reader(
            Some(&global_path),
            &project_path,
            |path| {
                if path == global_path {
                    global_reads.set(global_reads.get() + 1);
                    Ok(Some(global_generation.to_string()))
                } else if path == project_path {
                    project_reads.set(project_reads.get() + 1);
                    Ok(Some(captured_project_generation.to_string()))
                } else {
                    panic!("unexpected config path: {}", path.display());
                }
            },
        )
        .unwrap();

        assert_eq!(global_reads.get(), 1);
        assert_eq!(project_reads.get(), 1);
        let admission = snapshot
            .model_catalog
            .validate_parts("codex", "openai", "gpt-future-snapshot", "high")
            .expect("captured generation has no tombstone");
        assert!(matches!(
            admission.provenance,
            CatalogProvenance::Global { ref path, ref key }
                if path == &global_path && key == "preferences.primary_writer_spec"
        ));
    }
}
