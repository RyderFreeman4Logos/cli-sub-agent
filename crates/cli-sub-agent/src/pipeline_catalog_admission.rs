use anyhow::Result;
use csa_executor::{Executor, ModelSpec};

#[derive(Debug)]
pub(super) struct ValidatedExecutorIdentity {
    pub(super) resolved_model_spec: ModelSpec,
    pub(super) catalog_admission: csa_config::CatalogAdmission,
}

#[cfg(test)]
impl ValidatedExecutorIdentity {
    pub(super) fn expect(self, _message: &str) -> csa_config::CatalogAdmission {
        self.catalog_admission
    }
}

pub(super) fn validate_final_executor_identity(
    executor: &Executor,
    original_model_spec: Option<&str>,
    final_model_request: Option<&str>,
    model_catalog: &csa_config::EffectiveModelCatalog,
) -> Result<ValidatedExecutorIdentity> {
    let original = original_model_spec
        .map(csa_executor::ModelSpec::parse)
        .transpose()?;
    let requested_full_spec = final_model_request
        .filter(|value| value.matches('/').count() == 3)
        .map(csa_executor::ModelSpec::parse)
        .transpose()?;
    if let Some(spec) = requested_full_spec.as_ref()
        && spec.tool != executor.tool_name()
    {
        anyhow::bail!(
            "execution-boundary catalog rejection: selected tool {} cannot admit model spec for tool {}",
            executor.tool_name(),
            spec.tool
        );
    }
    let requested_model = final_model_request.map(|value| {
        csa_executor::ThinkingBudget::try_split_from_model(value)
            .0
            .to_string()
    });
    let raw_model = requested_model
        .as_deref()
        .or_else(|| original.as_ref().map(|spec| spec.model.as_str()))
        .or_else(|| executor.model_override())
        .unwrap_or("default");
    let (provider, model) = if let Some(spec) = requested_full_spec.as_ref() {
        (spec.provider.clone(), spec.model.clone())
    } else if let Some((provider, model)) = raw_model.split_once('/') {
        (provider.to_string(), model.to_string())
    } else if final_model_request.is_some()
        && let Some(spec) = original.as_ref()
    {
        (spec.provider.clone(), raw_model.to_string())
    } else if let Some(provider) = executor.provider_override() {
        (provider.to_string(), raw_model.to_string())
    } else if let Some(spec) = original.as_ref() {
        (spec.provider.clone(), raw_model.to_string())
    } else {
        (
            model_catalog
                .resolve_provider_for_model(executor.tool_name(), raw_model)
                .map_err(|error| {
                    anyhow::anyhow!("execution-boundary catalog rejection: {error}")
                })?,
            raw_model.to_string(),
        )
    };
    let final_spec = csa_executor::ModelSpec {
        tool: executor.tool_name().to_string(),
        provider,
        model,
        thinking_budget: executor
            .thinking_budget()
            .cloned()
            .unwrap_or(csa_executor::ThinkingBudget::DefaultBudget),
    };
    // The executor tool is already a parsed ToolName; the effective catalog is
    // authoritative for its provider/model/reasoning identity. Restrict this
    // call to the selected typed tool instead of a stale global tool list.
    let known_tools = [executor.tool_name()];
    let catalog_admission = final_spec
        .validate_with_catalog(model_catalog, &known_tools)
        .map_err(|error| anyhow::anyhow!("execution-boundary catalog rejection: {error}"))?;
    Ok(ValidatedExecutorIdentity {
        resolved_model_spec: final_spec,
        catalog_admission,
    })
}
