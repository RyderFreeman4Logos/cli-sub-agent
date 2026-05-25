use anyhow::Result;
use csa_config::ProjectConfig;
use csa_executor::{ModelSpec, ThinkingBudget};

pub(crate) fn enforce_model_spec_matches_tool_default(
    config: &ProjectConfig,
    parsed: &ModelSpec,
    raw_spec: &str,
) -> Result<()> {
    let Some(default_model) = config.tool_default_model(&parsed.tool) else {
        return Ok(());
    };
    let resolved_default = config.resolve_alias(default_model);
    let (configured_model, _) = ThinkingBudget::try_split_from_model(&resolved_default);
    let provider_model = format!("{}/{}", parsed.provider, parsed.model);
    if configured_model == parsed.model || configured_model == provider_model {
        return Ok(());
    }

    anyhow::bail!(
        "Model spec '{}' is not configured for tool '{}'. \
         Configured [tools.{}].default_model is '{}'. \
         Use the configured model, add this exact spec to a [tiers.*] section, \
         or pass --force-ignore-tier-setting to override.",
        raw_spec,
        parsed.tool,
        parsed.tool,
        default_model
    )
}
