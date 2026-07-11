use std::path::Path;

use anyhow::Result;
use csa_config::ProjectConfig;

use super::BatchTask;

pub(super) fn register_batch_model_specs(
    catalog: &mut csa_config::EffectiveModelCatalog,
    tasks: &[BatchTask],
    batch_path: &Path,
    project_config: Option<&ProjectConfig>,
    global_config: &csa_config::GlobalConfig,
    project_root: &Path,
) -> Result<()> {
    for (index, task) in tasks.iter().enumerate() {
        let Some(raw_model) = task.model.as_deref() else {
            continue;
        };
        validate_batch_model_identity(task, batch_path, index, raw_model)?;
        let slash_count = raw_model.matches('/').count();
        let (provider, model, suffix_reasoning) = if slash_count == 3 {
            let parsed = csa_executor::ModelSpec::parse(raw_model)?;
            if parsed.tool != task.tool {
                anyhow::bail!(
                    "batch task '{}' selects tool '{}' in model spec '{}' but task.tool is '{}'",
                    task.name,
                    parsed.tool,
                    raw_model,
                    task.tool
                );
            }
            let mut parts = raw_model.splitn(4, '/');
            let _tool = parts.next().expect("validated full model spec tool");
            let provider = parts.next().expect("validated full model spec provider");
            let model = parts.next().expect("validated full model spec model");
            let reasoning = parts.next().expect("validated full model spec reasoning");
            (provider.to_string(), model.to_string(), Some(reasoning))
        } else {
            let (normalized_model, suffix_budget) =
                csa_executor::ThinkingBudget::try_split_from_model(raw_model);
            let suffix_reasoning = suffix_budget
                .as_ref()
                .and_then(|_| raw_model.rsplit_once('/').map(|(_, reasoning)| reasoning));
            if slash_count == 2 && suffix_reasoning.is_none() {
                anyhow::bail!(
                    "batch task '{}' has invalid model '{}': expected model, provider/model, provider/model/reasoning, or tool/provider/model/reasoning",
                    task.name,
                    raw_model
                );
            }
            let (provider, model) =
                if let Some((provider, model)) = normalized_model.split_once('/') {
                    (provider.to_string(), model.to_string())
                } else {
                    (
                        catalog.resolve_provider_for_model(&task.tool, normalized_model)?,
                        normalized_model.to_string(),
                    )
                };
            (provider, model, suffix_reasoning)
        };
        let model_provenance = csa_config::CatalogProvenance::Inline {
            source: format!("batch config {}", batch_path.display()),
            key: format!("tasks[{index}].model"),
        };
        let project_lock = project_config.and_then(|config| config.thinking_lock(&task.tool));
        let global_lock = global_config.thinking_lock(&task.tool);
        let reasoning = project_lock
            .or(global_lock)
            .or(suffix_reasoning)
            .unwrap_or("default");
        let reasoning_provenance = if project_lock.is_some() {
            Some(csa_config::CatalogProvenance::Project {
                path: ProjectConfig::config_path(project_root),
                key: format!("tools.{}.thinking_lock", task.tool),
            })
        } else if global_lock.is_some() {
            Some(csa_config::CatalogProvenance::Global {
                path: csa_config::GlobalConfig::config_path()?,
                key: format!("tools.{}.thinking_lock", task.tool),
            })
        } else {
            None
        };
        if let Some(reasoning_provenance) = reasoning_provenance {
            catalog.register_configured_spec_with_reasoning_source(
                &task.tool,
                &provider,
                &model,
                reasoning,
                model_provenance,
                reasoning_provenance,
            )?;
        } else {
            catalog.register_configured_spec(
                &task.tool,
                &provider,
                &model,
                reasoning,
                model_provenance,
            )?;
        }
        catalog.validate_parts(&task.tool, &provider, &model, reasoning)?;
    }
    Ok(())
}

fn validate_batch_model_identity(
    task: &BatchTask,
    batch_path: &Path,
    index: usize,
    raw_model: &str,
) -> Result<()> {
    let source = format!("batch config {} tasks[{index}].model", batch_path.display());
    for component in raw_model.split('/') {
        if component.trim().is_empty() {
            anyhow::bail!(
                "batch task '{}' has an empty model identity component at {source}",
                task.name
            );
        }
        if component != component.trim() {
            anyhow::bail!(
                "batch task '{}' model identity component contains leading/trailing whitespace at {source}",
                task.name
            );
        }
    }
    Ok(())
}
