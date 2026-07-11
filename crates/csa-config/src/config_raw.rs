use anyhow::{Context, Result};
use std::path::Path;

use crate::config_merge::{reject_project_tier_policy, warn_deprecated_keys};

pub(crate) fn pruned_project_config_str(content: String, path: &Path) -> Result<String> {
    let Ok(mut raw) = toml::from_str::<toml::Value>(&content) else {
        return Ok(content);
    };
    warn_deprecated_keys(&raw, &path.display().to_string());
    prune_project_removed_refs(&mut raw, path);
    reject_project_tier_policy(&raw, &path.display().to_string())
        .with_context(|| format!("Invalid config: {}", path.display()))?;
    crate::validate::validate_tool_transport_overrides_in_raw_config(&raw)
        .with_context(|| format!("Invalid config: {}", path.display()))?;
    toml::to_string(&raw)
        .with_context(|| format!("Failed to serialize project config: {}", path.display()))
}

pub(crate) fn reject_removed_refs(raw: &toml::Value, path: &Path, label: &str) -> Result<()> {
    crate::validate::reject_removed_gemini_cli_in_raw_config(raw, &path.display().to_string())
        .with_context(|| format!("Invalid {label}config"))
}

pub(crate) fn prune_project_removed_refs(raw: &mut toml::Value, path: &Path) -> usize {
    crate::project_prune::prune_removed_project_refs_in_raw_config(raw, &path.display().to_string())
}
