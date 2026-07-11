use super::{
    ProjectConfig, enforce_global_tool_disables, merge_toml_values, prune_project_removed_refs,
    pruned_project_config_str, reject_project_tier_policy, reject_removed_refs,
    strip_review_project_only_from_global, warn_deprecated_keys,
};
use anyhow::{Context, Result};
use std::path::Path;

impl ProjectConfig {
    pub(crate) fn load_from_captured_sources(
        user_path: Option<&Path>,
        user_content: Option<&str>,
        project_path: &Path,
        project_content: Option<&str>,
    ) -> Result<Option<Self>> {
        match (user_path.zip(user_content), project_content) {
            (None, None) => Ok(None),
            (Some((path, content)), None) => Self::parse_user_contents(path, content),
            (None, Some(content)) => Self::parse_project_contents(project_path, content),
            (Some((base_path, base_content)), Some(project_content)) => {
                Self::parse_merged_contents(base_path, base_content, project_path, project_content)
            }
        }
    }

    pub(super) fn parse_user_contents(path: &Path, content: &str) -> Result<Option<Self>> {
        if let Ok(raw) = toml::from_str::<toml::Value>(content) {
            warn_deprecated_keys(&raw, &path.display().to_string());
            reject_removed_refs(&raw, path, "")?;
            crate::validate::validate_tool_transport_overrides_in_raw_config(&raw)
                .with_context(|| format!("Invalid config: {}", path.display()))?;
        }
        let mut config: Self = toml::from_str(content)
            .with_context(|| format!("Failed to parse config: {}", path.display()))?;
        config.sanitize_filesystem_sandbox();
        crate::validate::validate_tool_transport_overrides(&config)?;
        Ok(Some(config))
    }

    pub(super) fn parse_project_contents(path: &Path, content: &str) -> Result<Option<Self>> {
        let config_str = pruned_project_config_str(content.to_string(), path)?;
        let mut config: Self = toml::from_str(&config_str)
            .with_context(|| format!("Failed to parse config: {}", path.display()))?;
        config.sanitize_filesystem_sandbox();
        crate::validate::validate_tool_transport_overrides(&config)?;
        Ok(Some(config))
    }

    pub(super) fn parse_merged_contents(
        base_path: &Path,
        base_str: &str,
        overlay_path: &Path,
        overlay_str: &str,
    ) -> Result<Option<Self>> {
        let base_val: toml::Value = toml::from_str(base_str)
            .with_context(|| format!("Failed to parse user config: {}", base_path.display()))?;
        let mut overlay_val: toml::Value = toml::from_str(overlay_str).with_context(|| {
            format!("Failed to parse project config: {}", overlay_path.display())
        })?;

        warn_deprecated_keys(&base_val, &base_path.display().to_string());
        warn_deprecated_keys(&overlay_val, &overlay_path.display().to_string());
        reject_removed_refs(&base_val, base_path, "user ")?;
        prune_project_removed_refs(&mut overlay_val, overlay_path);
        reject_project_tier_policy(&overlay_val, &overlay_path.display().to_string())
            .with_context(|| format!("Invalid project config: {}", overlay_path.display()))?;
        crate::validate::validate_tool_transport_overrides_in_raw_config(&base_val)
            .with_context(|| format!("Invalid user config: {}", base_path.display()))?;
        crate::validate::validate_tool_transport_overrides_in_raw_config(&overlay_val)
            .with_context(|| format!("Invalid project config: {}", overlay_path.display()))?;

        let base_schema = base_val.get("schema_version").and_then(|v| v.as_integer());
        let overlay_schema = overlay_val
            .get("schema_version")
            .and_then(|v| v.as_integer());

        let mut base_for_merge = base_val.clone();
        strip_review_project_only_from_global(&mut base_for_merge);

        let mut merged = merge_toml_values(base_for_merge, overlay_val);
        if let Some(max_ver) = match (base_schema, overlay_schema) {
            (Some(b), Some(o)) => Some(b.max(o)),
            (Some(v), None) | (None, Some(v)) => Some(v),
            (None, None) => None,
        } && let toml::Value::Table(ref mut table) = merged
        {
            table.insert("schema_version".to_string(), toml::Value::Integer(max_ver));
        }

        enforce_global_tool_disables(&base_val, &mut merged);
        crate::validate::validate_tool_transport_overrides_in_raw_config(&merged)
            .context("Invalid merged config after layering")?;

        let merged_str = toml::to_string(&merged).context("Failed to serialize merged config")?;
        let mut config: Self =
            toml::from_str(&merged_str).context("Failed to deserialize merged config")?;
        config.sanitize_filesystem_sandbox();
        crate::validate::validate_tool_transport_overrides(&config)?;
        Ok(Some(config))
    }
}
