use crate::{CatalogProvenance, EffectiveModelCatalog, ProjectConfig, ReviewConfig};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// Register every model identity that the effective configuration can select.
///
/// Configuration is authoritative for future backend slugs, but exact catalog
/// tombstones remain authoritative during admission. Registration happens once
/// while the immutable command snapshot is assembled.
pub(crate) fn register_configured_specs(
    catalog: &mut EffectiveModelCatalog,
    config: &ProjectConfig,
    global_path: Option<&Path>,
    project_path: &Path,
    project_raw: Option<toml::Value>,
) -> Result<()> {
    let sources = ConfiguredSources::new(global_path, project_path, project_raw);

    for (tier_name, tier) in &config.tiers {
        for (index, spec) in tier.models.iter().enumerate() {
            let key = format!("tiers.{tier_name}.models[{index}]");
            let provenance = sources.provenance(&key, &["tiers", tier_name.as_str(), "models"]);
            register_full_spec(catalog, spec, provenance.clone(), &key)?;

            if let Some(tool) = spec.split('/').next()
                && let Some(reasoning) = config.thinking_lock(tool)
            {
                register_full_spec_with_reasoning(
                    catalog,
                    spec,
                    reasoning,
                    provenance,
                    sources.provenance(
                        &format!("tools.{tool}.thinking_lock"),
                        &["tools", tool, "thinking_lock"],
                    ),
                    &key,
                )?;
            }
        }
    }

    if let Some(preferences) = config.preferences.as_ref()
        && let Some(spec) = preferences.primary_writer_spec.as_deref()
    {
        let key = "preferences.primary_writer_spec";
        let provenance = sources.provenance(key, &["preferences", "primary_writer_spec"]);
        register_full_spec(catalog, spec, provenance.clone(), key)?;
        if let Some(tool) = spec.split('/').next()
            && let Some(reasoning) = config.thinking_lock(tool)
        {
            register_full_spec_with_reasoning(
                catalog,
                spec,
                reasoning,
                provenance,
                sources.provenance(
                    &format!("tools.{tool}.thinking_lock"),
                    &["tools", tool, "thinking_lock"],
                ),
                key,
            )?;
        }
    }

    for (alias, value) in &config.aliases {
        if value.matches('/').count() < 3 {
            continue;
        }
        let key = format!("aliases.{alias}");
        let provenance = sources.provenance(&key, &["aliases", alias.as_str()]);
        register_full_spec(catalog, value, provenance.clone(), &key)?;
        if let Some(tool) = value.split('/').next()
            && let Some(reasoning) = config.thinking_lock(tool)
        {
            register_full_spec_with_reasoning(
                catalog,
                value,
                reasoning,
                provenance,
                sources.provenance(
                    &format!("tools.{tool}.thinking_lock"),
                    &["tools", tool, "thinking_lock"],
                ),
                &key,
            )?;
        }
    }

    for (tool, tool_config) in &config.tools {
        if !tool_config.enabled {
            continue;
        }
        let Some(default_model) = tool_config.default_model.as_deref() else {
            continue;
        };
        let resolved_model = config.resolve_alias(default_model);
        let (reasoning, reasoning_provenance) =
            if let Some(lock) = tool_config.thinking_lock.as_deref() {
                (
                    Some(lock),
                    Some(sources.provenance(
                        &format!("tools.{tool}.thinking_lock"),
                        &["tools", tool.as_str(), "thinking_lock"],
                    )),
                )
            } else if let Some(default) = tool_config.default_thinking.as_deref() {
                (
                    Some(default),
                    Some(sources.provenance(
                        &format!("tools.{tool}.default_thinking"),
                        &["tools", tool.as_str(), "default_thinking"],
                    )),
                )
            } else {
                (None, None)
            };
        let key = format!("tools.{tool}.default_model");
        register_model_selection(
            catalog,
            tool,
            &resolved_model,
            reasoning,
            sources.provenance(&key, &["tools", tool.as_str(), "default_model"]),
            reasoning_provenance,
            &key,
        )?;
    }

    if let Some(review) = config.review.as_ref()
        && review.tier.is_none()
    {
        register_operation_model(catalog, config, review, "review", &sources)?;
    }
    if let Some(debate) = config.debate.as_ref()
        && debate.tier.is_none()
    {
        register_operation_model(catalog, config, debate, "debate", &sources)?;
    }

    Ok(())
}

#[cfg(test)]
pub(crate) fn register_configured_tier_specs(
    catalog: &mut EffectiveModelCatalog,
    config: &ProjectConfig,
    project_path: &Path,
) -> Result<()> {
    register_configured_specs(catalog, config, None, project_path, None)
}

fn register_operation_model(
    catalog: &mut EffectiveModelCatalog,
    config: &ProjectConfig,
    operation: &ReviewConfig,
    section: &str,
    sources: &ConfiguredSources,
) -> Result<()> {
    let Some(model) = operation.model.as_deref() else {
        return Ok(());
    };
    let resolved_model = config.resolve_alias(model);
    let key = format!("{section}.model");
    let provenance = sources.provenance(&key, &[section, "model"]);

    if resolved_model.matches('/').count() == 3 {
        let [tool, _, _, _] = parse_spec(&key, &resolved_model)?;
        let configured_tools = operation.tool.preference_order();
        if !configured_tools.is_empty() && !configured_tools.iter().any(|value| value == tool) {
            bail!(
                "Configured model at {key} selects tool '{tool}' but {section}.tool does not allow it"
            );
        }
        let (reasoning, reasoning_provenance) =
            operation_reasoning(config, operation, tool, section, sources);
        return register_model_selection(
            catalog,
            tool,
            &resolved_model,
            reasoning,
            provenance,
            reasoning_provenance,
            &key,
        );
    }

    let mut tools = operation.tool.preference_order();
    if tools.is_empty() {
        tools = crate::global::routing_candidate_tools()
            .iter()
            .map(|tool| tool.as_str().to_string())
            .filter(|tool| config.is_tool_enabled(tool))
            .collect();
    }
    for tool in tools {
        let (reasoning, reasoning_provenance) =
            operation_reasoning(config, operation, &tool, section, sources);
        register_model_selection(
            catalog,
            &tool,
            &resolved_model,
            reasoning,
            provenance.clone(),
            reasoning_provenance,
            &key,
        )?;
    }
    Ok(())
}

fn operation_reasoning<'a>(
    config: &'a ProjectConfig,
    operation: &'a ReviewConfig,
    tool: &str,
    section: &str,
    sources: &ConfiguredSources,
) -> (Option<&'a str>, Option<CatalogProvenance>) {
    if let Some(reasoning) = config.thinking_lock(tool) {
        return (
            Some(reasoning),
            Some(sources.provenance(
                &format!("tools.{tool}.thinking_lock"),
                &["tools", tool, "thinking_lock"],
            )),
        );
    }
    if let Some(reasoning) = operation.thinking.as_deref() {
        return (
            Some(reasoning),
            Some(sources.provenance(&format!("{section}.thinking"), &[section, "thinking"])),
        );
    }
    (None, None)
}

fn register_model_selection(
    catalog: &mut EffectiveModelCatalog,
    expected_tool: &str,
    raw_model: &str,
    reasoning_override: Option<&str>,
    model_provenance: CatalogProvenance,
    reasoning_provenance: Option<CatalogProvenance>,
    key: &str,
) -> Result<()> {
    validate_identity_whitespace(key, "model value", raw_model)?;
    if raw_model.matches('/').count() == 3 {
        let [tool, provider, model, embedded_reasoning] = parse_spec(key, raw_model)?;
        if tool != expected_tool {
            bail!(
                "Configured model at {key} selects tool '{tool}' but the effective tool is '{expected_tool}'"
            );
        }
        let reasoning = reasoning_override.unwrap_or(embedded_reasoning);
        validate_selected_identity(key, tool, provider, model, reasoning)?;
        return register_selected_parts(
            catalog,
            tool,
            provider,
            model,
            reasoning,
            model_provenance,
            reasoning_provenance,
        );
    }
    if raw_model.matches('/').count() > 3 {
        bail!(
            "Configured model at {key} has invalid value '{raw_model}': expected model, provider/model, or tool/provider/model/reasoning"
        );
    }

    let (model_with_provider, suffix_reasoning) = split_named_reasoning(raw_model);
    if raw_model.matches('/').count() == 2 && suffix_reasoning.is_none() {
        bail!(
            "Configured model at {key} has invalid value '{raw_model}': expected model, provider/model, provider/model/reasoning, or tool/provider/model/reasoning"
        );
    }
    let (provider, model) = if let Some((provider, model)) = model_with_provider.split_once('/') {
        (provider.to_string(), model.to_string())
    } else {
        (
            catalog
                .resolve_provider_for_model(expected_tool, model_with_provider)
                .map_err(|error| anyhow::anyhow!("Configured model at {key}: {error}"))?,
            model_with_provider.to_string(),
        )
    };
    let reasoning = reasoning_override.or(suffix_reasoning).unwrap_or("default");
    validate_selected_identity(key, expected_tool, &provider, &model, reasoning)?;
    register_selected_parts(
        catalog,
        expected_tool,
        &provider,
        &model,
        reasoning,
        model_provenance,
        reasoning_provenance,
    )
}

fn register_selected_parts(
    catalog: &mut EffectiveModelCatalog,
    tool: &str,
    provider: &str,
    model: &str,
    reasoning: &str,
    model_provenance: CatalogProvenance,
    reasoning_provenance: Option<CatalogProvenance>,
) -> Result<()> {
    if let Some(reasoning_provenance) = reasoning_provenance {
        catalog.register_configured_spec_with_reasoning_source(
            tool,
            provider,
            model,
            reasoning,
            model_provenance,
            reasoning_provenance,
        )?;
    } else {
        catalog.register_configured_spec(tool, provider, model, reasoning, model_provenance)?;
    }
    Ok(())
}

fn register_full_spec(
    catalog: &mut EffectiveModelCatalog,
    spec: &str,
    provenance: CatalogProvenance,
    key: &str,
) -> Result<()> {
    let [tool, provider, model, reasoning] = parse_spec(key, spec)?;
    catalog.register_configured_spec(tool, provider, model, reasoning, provenance)?;
    Ok(())
}

fn register_full_spec_with_reasoning(
    catalog: &mut EffectiveModelCatalog,
    spec: &str,
    reasoning: &str,
    model_provenance: CatalogProvenance,
    reasoning_provenance: CatalogProvenance,
    key: &str,
) -> Result<()> {
    let [tool, provider, model, _] = parse_spec(key, spec)?;
    catalog.register_configured_spec_with_reasoning_source(
        tool,
        provider,
        model,
        reasoning,
        model_provenance,
        reasoning_provenance,
    )?;
    Ok(())
}

fn parse_spec<'a>(key: &str, spec: &'a str) -> Result<[&'a str; 4]> {
    let parts: Vec<&str> = spec.split('/').collect();
    let [tool, provider, model, reasoning] = parts.as_slice() else {
        bail!(
            "Configured model at {key} has invalid model spec '{spec}'. Expected format: 'tool/provider/model/reasoning'"
        );
    };
    for (component, value) in [
        ("tool", *tool),
        ("provider", *provider),
        ("model", *model),
        ("reasoning", *reasoning),
    ] {
        validate_identity_whitespace(key, component, value)?;
    }
    if csa_core::types::is_removed_tool_name(tool) {
        bail!(
            "Configured model at {key} references removed tool '{tool}'. {}",
            csa_core::types::removed_tool_error("gemini-cli")
        );
    }
    if !crate::global::all_known_tools()
        .iter()
        .any(|known| known.as_str() == *tool)
    {
        bail!("Configured model at {key} has unknown tool '{tool}'");
    }
    Ok([tool, provider, model, reasoning])
}

fn validate_selected_identity(
    key: &str,
    tool: &str,
    provider: &str,
    model: &str,
    reasoning: &str,
) -> Result<()> {
    for (component, value) in [
        ("tool", tool),
        ("provider", provider),
        ("model", model),
        ("reasoning", reasoning),
    ] {
        validate_identity_whitespace(key, component, value)?;
    }
    Ok(())
}

fn validate_identity_whitespace(key: &str, component: &str, value: &str) -> Result<()> {
    if value.trim() != value {
        bail!(
            "Configured model at {key} has invalid {component} '{value}': leading or trailing whitespace is not allowed"
        );
    }
    Ok(())
}

fn split_named_reasoning(value: &str) -> (&str, Option<&str>) {
    let Some((model, suffix)) = value.rsplit_once('/') else {
        return (value, None);
    };
    if csa_core::model_catalog::ReasoningEffort::parse(suffix).is_some() {
        (model, Some(suffix))
    } else {
        (value, None)
    }
}

struct ConfiguredSources {
    global_path: Option<PathBuf>,
    project_path: PathBuf,
    project_raw: Option<toml::Value>,
}

impl ConfiguredSources {
    fn new(
        global_path: Option<&Path>,
        project_path: &Path,
        project_raw: Option<toml::Value>,
    ) -> Self {
        Self {
            global_path: global_path.map(Path::to_path_buf),
            project_path: project_path.to_path_buf(),
            project_raw,
        }
    }

    fn provenance(&self, key: &str, raw_path: &[&str]) -> CatalogProvenance {
        if self.project_declares(raw_path) || self.global_path.is_none() {
            CatalogProvenance::Project {
                path: self.project_path.clone(),
                key: key.to_string(),
            }
        } else {
            CatalogProvenance::Global {
                path: self
                    .global_path
                    .clone()
                    .expect("global path checked as present"),
                key: key.to_string(),
            }
        }
    }

    fn project_declares(&self, path: &[&str]) -> bool {
        let Some(mut value) = self.project_raw.as_ref() else {
            return false;
        };
        for segment in path {
            let Some(next) = value.get(*segment) else {
                return false;
            };
            value = next;
        }
        true
    }
}
