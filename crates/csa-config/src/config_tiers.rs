//! Tier whitelist enforcement methods for `ProjectConfig`.
//!
//! These methods validate that tool/model/thinking selections conform to the
//! tier definitions in `.csa/config.toml`. Extracted from `config.rs` for
//! module size management.

use crate::config::ProjectConfig;

// Re-import types needed by test submodule (via `use super::*`).
#[cfg(test)]
use crate::config::{CURRENT_SCHEMA_VERSION, ProjectMeta, TierConfig};
#[cfg(test)]
use crate::config_resources::ResourcesConfig;
#[cfg(test)]
use std::collections::HashMap;

impl ProjectConfig {
    /// Check whether a full model spec string appears in any tier's models list.
    ///
    /// Performs exact string match against all tier model specs.
    pub fn is_model_spec_in_tiers(&self, spec: &str) -> bool {
        self.tiers
            .values()
            .any(|tier| tier.models.iter().any(|m| m == spec))
    }

    /// Return tier models filtered to only include enabled tools.
    ///
    /// For each tier, model specs whose tool component (first `/`-delimited
    /// segment) maps to a disabled tool are excluded. Useful for display
    /// commands (`csa tiers list`, `csa config show --effective`) where the
    /// user expects to see only actionable entries.
    pub fn enabled_tier_models(&self, tier_name: &str) -> Vec<String> {
        let Some(tier) = self.tiers.get(tier_name) else {
            return Vec::new();
        };
        tier.models
            .iter()
            .filter(|spec| {
                spec.split('/')
                    .next()
                    .is_some_and(|tool| self.is_tool_enabled(tool))
            })
            .cloned()
            .collect()
    }

    /// Return all model specs from tiers that use the given tool.
    ///
    /// Useful for error messages showing which specs are allowed.
    pub fn allowed_model_specs_for_tool(&self, tool: &str) -> Vec<String> {
        self.tiers
            .values()
            .flat_map(|tier| tier.models.iter())
            .filter(|spec| spec.split('/').next().is_some_and(|t| t == tool))
            .cloned()
            .collect()
    }

    /// Enforce tier whitelist: reject tool/model combinations not in tiers.
    ///
    /// When tiers are configured (non-empty), any explicit tool or model-spec
    /// must appear in at least one tier. This prevents accidental use of
    /// unplanned tools that could exhaust subscription quotas.
    ///
    /// Returns `Ok(())` when:
    /// - tiers is empty (no restriction — backward compatible)
    /// - tool appears in at least one tier model spec
    /// - model_spec (if provided) exactly matches a tier model spec
    pub fn enforce_tier_whitelist(
        &self,
        tool: &str,
        model_spec: Option<&str>,
    ) -> anyhow::Result<()> {
        // Empty tiers = no restriction (backward compatible)
        if self.tiers.is_empty() {
            return Ok(());
        }

        // Tool must appear in at least one tier
        if !self.is_tool_configured_in_tiers(tool) {
            let configured_tools: Vec<String> = crate::global::all_known_tools()
                .iter()
                .filter(|t| self.is_tool_configured_in_tiers(t.as_str()))
                .map(|t| t.as_str().to_string())
                .collect();
            anyhow::bail!(
                "Tool '{}' is not configured in any tier. \
                 Configured tools: [{}]. \
                 Add it to a [tiers.*] section or use a configured tool.",
                tool,
                configured_tools.join(", ")
            );
        }

        // If model_spec provided, verify tool/spec consistency and tier membership
        if let Some(spec) = model_spec {
            // Cross-field consistency: spec's tool component must match selected tool
            if let Some(spec_tool) = spec.split('/').next() {
                if spec_tool != tool {
                    anyhow::bail!(
                        "Model spec '{}' belongs to tool '{}', not '{}'. \
                         Use --tool {} or select a spec for '{}'.",
                        spec,
                        spec_tool,
                        tool,
                        spec_tool,
                        tool
                    );
                }
            }
            if !self.is_model_spec_in_tiers(spec) {
                let allowed = self.allowed_model_specs_for_tool(tool);
                anyhow::bail!(
                    "Model spec '{}' is not configured in any tier. \
                     Allowed specs for '{}': [{}]. \
                     Add it to a [tiers.*] section or use a configured spec.",
                    spec,
                    tool,
                    allowed.join(", ")
                );
            }
        }

        Ok(())
    }

    /// Check if a model name appears in any tier spec for the given tool.
    ///
    /// Model specs have format `tool/provider/model/thinking_budget`.
    /// Supports two model name formats:
    /// - Bare model: `gemini-2.5-pro` → matches spec's 3rd component
    /// - Provider/model: `google/gemini-2.5-pro` → matches spec's 2nd+3rd components
    pub fn is_model_name_in_tiers_for_tool(&self, tool: &str, model_name: &str) -> bool {
        let name_parts: Vec<&str> = model_name.splitn(2, '/').collect();
        self.tiers.values().any(|tier| {
            tier.models.iter().any(|spec| {
                let parts: Vec<&str> = spec.splitn(4, '/').collect();
                if parts.len() < 3 || parts[0] != tool {
                    return false;
                }
                if name_parts.len() == 2 {
                    // Provider/model format: match provider + model components
                    parts[1] == name_parts[0] && parts[2] == name_parts[1]
                } else {
                    // Bare model name: match model component only
                    parts[2] == model_name
                }
            })
        })
    }

    /// Enforce that a model name (from `--model`) is configured in tiers for the tool.
    ///
    /// Only enforced when tiers are non-empty. Skips check when model_name is None.
    pub fn enforce_tier_model_name(
        &self,
        tool: &str,
        model_name: Option<&str>,
    ) -> anyhow::Result<()> {
        if self.tiers.is_empty() {
            return Ok(());
        }
        let Some(name) = model_name else {
            return Ok(());
        };
        // If the "model name" is actually a full model spec (4-part: tool/provider/model/budget),
        // delegate to the spec-level check instead. This handles aliases that
        // resolve to full specs like "codex/openai/gpt-5.3-codex/high".
        // Only match exactly 4 parts — provider/model formats like "google/gemini-2.5-pro"
        // (2 parts) should fall through to the model-name check below.
        if name.split('/').count() == 4 {
            return self.enforce_tier_whitelist(tool, Some(name));
        }
        if !self.is_model_name_in_tiers_for_tool(tool, name) {
            let allowed_specs = self.allowed_model_specs_for_tool(tool);
            let allowed_models: Vec<String> = allowed_specs
                .iter()
                .filter_map(|spec| {
                    let parts: Vec<&str> = spec.splitn(4, '/').collect();
                    if parts.len() >= 3 {
                        Some(format!("{} (or {}/{})", parts[2], parts[1], parts[2]))
                    } else {
                        None
                    }
                })
                .collect();
            anyhow::bail!(
                "Model '{}' for tool '{}' is not configured in any tier. \
                 Allowed models for '{}': [{}]. \
                 Add it to a [tiers.*] section or use a configured model.",
                name,
                tool,
                tool,
                allowed_models.join(", ")
            );
        }
        Ok(())
    }

    /// Enforce that a `--thinking` level appears in at least one tier's model spec.
    ///
    /// Model specs have format `tool/provider/model/thinking_budget`.
    /// This checks the 4th component against the requested thinking level.
    ///
    /// Only enforced when tiers are non-empty. Skips when `thinking` is `None`.
    pub fn enforce_thinking_level(&self, thinking: Option<&str>) -> anyhow::Result<()> {
        if self.tiers.is_empty() {
            return Ok(());
        }
        let Some(level) = thinking else {
            return Ok(());
        };
        let level_lower = level.to_ascii_lowercase();
        let found = self.tiers.values().any(|tier| {
            tier.models.iter().any(|spec| {
                spec.splitn(4, '/')
                    .nth(3)
                    .is_some_and(|t| t.to_ascii_lowercase() == level_lower)
            })
        });
        if !found {
            let configured_levels: Vec<String> = {
                let mut levels = std::collections::BTreeSet::new();
                for tier in self.tiers.values() {
                    for spec in &tier.models {
                        if let Some(t) = spec.splitn(4, '/').nth(3) {
                            levels.insert(t.to_string());
                        }
                    }
                }
                levels.into_iter().collect()
            };
            anyhow::bail!(
                "Thinking level '{}' is not configured in any tier. \
                 Configured levels: [{}]. \
                 Add it to a [tiers.*] model spec or use --force-override-user-config.",
                level,
                configured_levels.join(", ")
            );
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "config_tests_tier_whitelist.rs"]
mod tests;
