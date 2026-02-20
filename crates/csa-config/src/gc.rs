use crate::ProjectConfig;
use crate::config_merge::merge_toml_values;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct GcConfig {
    #[serde(default = "default_transcript_max_age_days")]
    pub transcript_max_age_days: u64,
    #[serde(default = "default_transcript_max_size_mb")]
    pub transcript_max_size_mb: u64,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            transcript_max_age_days: default_transcript_max_age_days(),
            transcript_max_size_mb: default_transcript_max_size_mb(),
        }
    }
}

impl GcConfig {
    pub fn is_default(&self) -> bool {
        self.transcript_max_age_days == default_transcript_max_age_days()
            && self.transcript_max_size_mb == default_transcript_max_size_mb()
    }

    /// Load effective GC config for a project.
    ///
    /// Merge precedence follows ProjectConfig:
    /// user config (base) < project config (overlay).
    pub fn load_for_project(project_root: &Path) -> Result<Self> {
        let project_path = project_root.join(".csa").join("config.toml");
        let user_path = ProjectConfig::user_config_path();
        let project_exists = project_path.exists();
        let user_exists = user_path.as_ref().is_some_and(|p| p.exists());

        if !project_exists && !user_exists {
            return Ok(Self::default());
        }

        let mut merged: Option<toml::Value> = None;
        if let Some(path) = user_path.as_deref().filter(|p| p.exists()) {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read user config: {}", path.display()))?;
            let raw: toml::Value = toml::from_str(&content)
                .with_context(|| format!("Failed to parse user config: {}", path.display()))?;
            merged = Some(raw);
        }

        if project_exists {
            let content = std::fs::read_to_string(&project_path).with_context(|| {
                format!("Failed to read project config: {}", project_path.display())
            })?;
            let raw: toml::Value = toml::from_str(&content).with_context(|| {
                format!("Failed to parse project config: {}", project_path.display())
            })?;
            merged = Some(match merged {
                Some(base) => merge_toml_values(base, raw),
                None => raw,
            });
        }

        let merged = merged.unwrap_or(toml::Value::Table(toml::map::Map::new()));
        let envelope: GcConfigEnvelope =
            toml::from_str(&toml::to_string(&merged)?).context("Failed to decode [gc] config")?;
        Ok(envelope.gc)
    }
}

fn default_transcript_max_age_days() -> u64 {
    30
}

fn default_transcript_max_size_mb() -> u64 {
    500
}

#[derive(Debug, Default, Deserialize)]
struct GcConfigEnvelope {
    #[serde(default)]
    gc: GcConfig,
}

#[cfg(test)]
mod tests {
    use super::GcConfig;

    #[test]
    fn gc_defaults_match_expected_values() {
        let cfg = GcConfig::default();
        assert_eq!(cfg.transcript_max_age_days, 30);
        assert_eq!(cfg.transcript_max_size_mb, 500);
    }
}
