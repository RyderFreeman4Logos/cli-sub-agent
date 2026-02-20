use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::global::{PreferencesConfig, ReviewConfig};

/// Sandbox enforcement mode for resource limits (cgroups, rlimits).
///
/// Controls whether CSA enforces memory/PID limits on child tool processes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnforcementMode {
    /// Require sandbox setup; abort if kernel support is missing.
    Required,
    /// Try to enforce limits; fall back gracefully if unavailable.
    BestEffort,
    /// Disable sandbox enforcement entirely.
    #[default]
    Off,
}

/// Resource profile for a tool, determining default sandbox behavior.
///
/// Auto-assigned based on tool runtime characteristics. Lightweight tools
/// (native binaries) skip sandbox overhead; heavyweight tools (Node.js with
/// plugin loading) get enforced limits.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolResourceProfile {
    /// Native binary (Rust/Go). Predictable memory, no sandbox needed.
    /// Default enforcement: `Off`, no memory/swap limits.
    Lightweight,
    /// Node.js or similar runtime with dynamic plugin loading.
    /// Default enforcement: `BestEffort` with memory and swap limits.
    #[default]
    Heavyweight,
    /// User-specified limits override all profile defaults.
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierConfig {
    pub description: String,
    pub models: Vec<String>,
    /// Optional token budget allocated for sessions using this tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    /// Optional maximum number of execution turns for this tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
}

/// Current schema version for config.toml
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub project: ProjectMeta,
    #[serde(default, skip_serializing_if = "ResourcesConfig::is_default")]
    pub resources: ResourcesConfig,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tools: HashMap<String, ToolConfig>,
    /// Optional per-project override for `csa review` tool selection.
    ///
    /// Example:
    /// ```toml
    /// [review]
    /// tool = "codex"  # or "claude-code"
    /// ```
    #[serde(default)]
    pub review: Option<ReviewConfig>,
    /// Optional per-project override for `csa debate` tool selection.
    ///
    /// Uses the same `ReviewConfig` shape (`tool = "auto" | "codex" | ...`).
    #[serde(default)]
    pub debate: Option<ReviewConfig>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tiers: HashMap<String, TierConfig>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tier_mapping: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub aliases: HashMap<String, String>,
    /// Optional per-project tool priority override.
    /// When set, overrides the global `[preferences].tool_priority`.
    #[serde(default)]
    pub preferences: Option<PreferencesConfig>,
}

fn default_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    #[serde(default = "default_project_name")]
    pub name: String,
    #[serde(default = "default_created_at")]
    pub created_at: DateTime<Utc>,
    #[serde(default = "default_recursion_depth")]
    pub max_recursion_depth: u32,
}

impl Default for ProjectMeta {
    fn default() -> Self {
        Self {
            name: default_project_name(),
            created_at: default_created_at(),
            max_recursion_depth: default_recursion_depth(),
        }
    }
}

fn default_project_name() -> String {
    "default".to_string()
}

fn default_created_at() -> DateTime<Utc> {
    Utc::now()
}

fn default_recursion_depth() -> u32 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub restrictions: Option<ToolRestrictions>,
    /// Suppress notification hooks (default: true). Injects `CSA_SUPPRESS_NOTIFY=1`.
    #[serde(default = "default_true")]
    pub suppress_notify: bool,
    /// Per-tool sandbox enforcement mode override. Takes precedence over project resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement_mode: Option<EnforcementMode>,
    /// Per-tool memory limit override (MB). Takes precedence over project resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_max_mb: Option<u64>,
    /// Per-tool swap limit override (MB). Takes precedence over project resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_swap_max_mb: Option<u64>,
    /// Per-tool Node.js heap size limit (MB). Takes precedence over project resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_heap_limit_mb: Option<u64>,
    /// Deprecated: use `setting_sources` instead.
    /// When `true`, equivalent to `setting_sources = []` (load nothing).
    /// When `false` or absent, no override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean_mode: Option<bool>,
    /// Selective MCP/setting sources to load for ACP-backed tools.
    /// `Some(vec![])` = load nothing (equivalent to old `lean_mode = true`).
    /// `Some(vec!["project"])` = load only project-level settings.
    /// `None` = default (load everything).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_sources: Option<Vec<String>>,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            restrictions: None,
            suppress_notify: true,
            enforcement_mode: None,
            memory_max_mb: None,
            memory_swap_max_mb: None,
            node_heap_limit_mb: None,
            lean_mode: None,
            setting_sources: None,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRestrictions {
    #[serde(default)]
    pub allow_edit_existing_files: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesConfig {
    /// Minimum combined free memory (physical + swap) in MB before refusing launch.
    #[serde(default = "default_min_mem")]
    pub min_free_memory_mb: u64,
    /// Kill child if no streamed output for this many consecutive seconds.
    #[serde(default = "default_idle_timeout_seconds")]
    pub idle_timeout_seconds: u64,
    #[serde(default)]
    pub initial_estimates: HashMap<String, u64>,
    /// Sandbox enforcement mode for resource limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement_mode: Option<EnforcementMode>,
    /// Maximum physical memory (RSS) in MB for child tool processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_max_mb: Option<u64>,
    /// Maximum swap usage in MB for child tool processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_swap_max_mb: Option<u64>,
    /// Default Node.js heap size limit (MB) for child tool processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_heap_limit_mb: Option<u64>,
    /// Maximum number of PIDs for child tool process trees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids_max: Option<u32>,
}

fn default_min_mem() -> u64 {
    4096
}

fn default_idle_timeout_seconds() -> u64 {
    120
}

impl Default for ResourcesConfig {
    fn default() -> Self {
        Self {
            min_free_memory_mb: default_min_mem(),
            idle_timeout_seconds: default_idle_timeout_seconds(),
            initial_estimates: HashMap::new(),
            enforcement_mode: None,
            memory_max_mb: None,
            memory_swap_max_mb: None,
            node_heap_limit_mb: None,
            pids_max: None,
        }
    }
}

impl ResourcesConfig {
    /// Returns true when all fields match their defaults.
    /// Used by `skip_serializing_if` to omit the `[resources]` section
    /// from minimal project configs.
    pub fn is_default(&self) -> bool {
        self.min_free_memory_mb == default_min_mem()
            && self.idle_timeout_seconds == default_idle_timeout_seconds()
            && self.initial_estimates.is_empty()
            && self.enforcement_mode.is_none()
            && self.memory_max_mb.is_none()
            && self.memory_swap_max_mb.is_none()
            && self.node_heap_limit_mb.is_none()
            && self.pids_max.is_none()
    }
}

/// Warn about deprecated config keys that serde silently ignores.
fn warn_deprecated_keys(raw: &toml::Value, source: &str) {
    if let Some(resources) = raw.get("resources") {
        if resources.get("min_free_swap_mb").is_some() {
            eprintln!(
                "warning: config '{}': 'resources.min_free_swap_mb' is deprecated and ignored. \
                 Use 'resources.min_free_memory_mb' (combined physical + swap threshold) instead.",
                source
            );
        }
    }
}

/// Deep merge two TOML values. Overlay wins for non-table values.
/// Tables are merged recursively (project-level keys override user-level keys).
fn merge_toml_values(base: toml::Value, overlay: toml::Value) -> toml::Value {
    match (base, overlay) {
        (toml::Value::Table(mut base_map), toml::Value::Table(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let merged_val = match base_map.remove(&key) {
                    Some(base_val) => merge_toml_values(base_val, overlay_val),
                    None => overlay_val,
                };
                base_map.insert(key, merged_val);
            }
            toml::Value::Table(base_map)
        }
        (_, overlay) => overlay,
    }
}

/// Re-apply `tools.*.enabled = false` from the global config into a merged
/// TOML value.  This ensures that global disablement is a hard override:
/// project configs cannot set a globally-disabled tool back to `enabled = true`.
fn enforce_global_tool_disables(global: &toml::Value, merged: &mut toml::Value) {
    let global_tools = match global.get("tools").and_then(|t| t.as_table()) {
        Some(t) => t,
        None => return,
    };
    let merged_tools = match merged.get_mut("tools").and_then(|t| t.as_table_mut()) {
        Some(t) => t,
        None => return,
    };

    for (tool_name, global_tool_val) in global_tools {
        let globally_disabled =
            global_tool_val.get("enabled").and_then(|v| v.as_bool()) == Some(false);
        if !globally_disabled {
            continue;
        }
        // Force `enabled = false` in the merged config for this tool.
        if let Some(merged_tool) = merged_tools.get_mut(tool_name) {
            if let Some(table) = merged_tool.as_table_mut() {
                table.insert("enabled".to_string(), toml::Value::Boolean(false));
            }
        } else {
            // Tool only in global config (already disabled via base merge), nothing to fix.
        }
    }
}

impl ProjectConfig {
    /// Load config with fallback chain:
    ///
    /// 1. If both `.csa/config.toml` (project) and `~/.config/cli-sub-agent/config.toml` (user)
    ///    exist, deep-merge them with project settings overriding user settings.
    /// 2. If only project config exists, use it directly.
    /// 3. If only user config exists, use it as fallback.
    /// 4. If neither exists, return None.
    pub fn load(project_root: &Path) -> Result<Option<Self>> {
        let project_path = project_root.join(".csa").join("config.toml");
        let user_path = Self::user_config_path();
        Self::load_with_paths(user_path.as_deref(), &project_path)
    }

    /// Load config from explicit paths. Testable without global filesystem state.
    ///
    /// `user_path`: path to user-level config (None if unavailable).
    /// `project_path`: path to project-level config.
    pub(crate) fn load_with_paths(
        user_path: Option<&Path>,
        project_path: &Path,
    ) -> Result<Option<Self>> {
        let project_exists = project_path.exists();
        let user_exists = user_path.is_some_and(|p| p.exists());

        match (user_exists, project_exists) {
            (false, false) => Ok(None),
            (true, false) => {
                // Safety: user_exists guarantees user_path is Some
                Self::load_from_path(user_path.unwrap())
            }
            (false, true) => Self::load_from_path(project_path),
            (true, true) => {
                // Safety: user_exists guarantees user_path is Some
                Self::load_merged(user_path.unwrap(), project_path)
            }
        }
    }

    fn load_from_path(path: &Path) -> Result<Option<Self>> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config: {}", path.display()))?;
        // Check for deprecated keys before deserializing (serde silently ignores them)
        if let Ok(raw) = content.parse::<toml::Value>() {
            warn_deprecated_keys(&raw, &path.display().to_string());
        }
        let config: Self = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config: {}", path.display()))?;
        Ok(Some(config))
    }

    /// Deep-merge user config (base) with project config (overlay).
    ///
    /// Uses `max(schema_version)` from both configs so that
    /// `check_schema_version()` catches incompatibility even when the
    /// project config has an older schema than the user config.
    fn load_merged(base_path: &Path, overlay_path: &Path) -> Result<Option<Self>> {
        let base_str = std::fs::read_to_string(base_path)
            .with_context(|| format!("Failed to read user config: {}", base_path.display()))?;
        let overlay_str = std::fs::read_to_string(overlay_path).with_context(|| {
            format!("Failed to read project config: {}", overlay_path.display())
        })?;

        let base_val: toml::Value = toml::from_str(&base_str)
            .with_context(|| format!("Failed to parse user config: {}", base_path.display()))?;
        let overlay_val: toml::Value = toml::from_str(&overlay_str).with_context(|| {
            format!("Failed to parse project config: {}", overlay_path.display())
        })?;

        // Check for deprecated keys in both configs
        warn_deprecated_keys(&base_val, &base_path.display().to_string());
        warn_deprecated_keys(&overlay_val, &overlay_path.display().to_string());

        // Preserve the higher schema_version before merging so that
        // check_schema_version() catches incompatibility from either source.
        // Only override when at least one file explicitly sets it; otherwise
        // let serde's `default_schema_version()` apply during deserialization.
        let base_schema = base_val.get("schema_version").and_then(|v| v.as_integer());
        let overlay_schema = overlay_val
            .get("schema_version")
            .and_then(|v| v.as_integer());

        let mut merged = merge_toml_values(base_val.clone(), overlay_val);
        // Set schema_version to max of both sources (only when at least one is explicit)
        if let Some(max_ver) = match (base_schema, overlay_schema) {
            (Some(b), Some(o)) => Some(b.max(o)),
            (Some(v), None) | (None, Some(v)) => Some(v),
            (None, None) => None,
        } {
            if let toml::Value::Table(ref mut table) = merged {
                table.insert("schema_version".to_string(), toml::Value::Integer(max_ver));
            }
        }

        // Global-disable-wins: re-apply `enabled = false` from the global (base)
        // config.  Global disablement is a hard override that project configs
        // cannot reverse — this prevents stale project configs from resurrecting
        // tools the user explicitly disabled at the global level.
        enforce_global_tool_disables(&base_val, &mut merged);

        // Roundtrip through string for reliable deserialization
        let merged_str = toml::to_string(&merged).context("Failed to serialize merged config")?;
        let config: Self =
            toml::from_str(&merged_str).context("Failed to deserialize merged config")?;
        Ok(Some(config))
    }

    /// Path to user-level config: `~/.config/cli-sub-agent/config.toml`.
    ///
    /// Returns None if the config directory cannot be determined
    /// (e.g., no HOME in containers).
    pub fn user_config_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "cli-sub-agent")
            .map(|dirs| dirs.config_dir().join("config.toml"))
    }

    /// Check if the config schema version is compatible with the current binary.
    /// Returns Ok(()) if compatible, or a descriptive error if migration is needed.
    pub fn check_schema_version(&self) -> Result<()> {
        if self.schema_version > CURRENT_SCHEMA_VERSION {
            anyhow::bail!(
                "Config schema version {} is newer than this binary supports (v{}).\n\
                 Please update CSA: csa self-update",
                self.schema_version,
                CURRENT_SCHEMA_VERSION
            );
        }
        // schema_version < CURRENT_SCHEMA_VERSION is fine — we maintain backward compatibility
        // Future migrations can be added here as needed
        Ok(())
    }

    /// Save config to .csa/config.toml
    pub fn save(&self, project_root: &Path) -> Result<()> {
        let config_dir = project_root.join(".csa");
        std::fs::create_dir_all(&config_dir)?;
        let config_path = config_dir.join("config.toml");
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, content)?;
        Ok(())
    }

    /// Check if a tool is enabled (unconfigured tools default to enabled)
    pub fn is_tool_enabled(&self, tool: &str) -> bool {
        self.tools.get(tool).map(|t| t.enabled).unwrap_or(true)
    }

    /// Check whether a tool appears in at least one tier model spec.
    pub fn is_tool_configured_in_tiers(&self, tool: &str) -> bool {
        self.tiers.values().any(|tier| {
            tier.models.iter().any(|model_spec| {
                model_spec
                    .split('/')
                    .next()
                    .is_some_and(|model_tool| model_tool == tool)
            })
        })
    }

    /// Check whether a tool is eligible for auto/heterogeneous selection.
    ///
    /// Rules:
    /// - Tool must be enabled (or unconfigured in `[tools]`, which defaults to enabled)
    /// - If tiers are configured, tool must appear in at least one tier model spec
    /// - If no tiers are configured (empty), all enabled tools are eligible
    pub fn is_tool_auto_selectable(&self, tool: &str) -> bool {
        self.is_tool_enabled(tool)
            && (self.tiers.is_empty() || self.is_tool_configured_in_tiers(tool))
    }

    /// Get the config file path for a project root
    pub fn config_path(project_root: &Path) -> std::path::PathBuf {
        project_root.join(".csa").join("config.toml")
    }

    /// Resolve tier-based tool selection for a given task type.
    ///
    /// Returns (tool_name, model_spec_string) for the first enabled tool in the tier.
    /// Falls back to tier3 if task_type not found in tier_mapping.
    /// Returns None if no enabled tools found.
    pub fn resolve_tier_tool(&self, task_type: &str) -> Option<(String, String)> {
        // 1. Look up task_type in tier_mapping to get tier name
        let tier_name = self
            .tier_mapping
            .get(task_type)
            .map(String::as_str)
            .or_else(|| {
                // Fallback: try to find tier3 or tier-3-*
                if self.tiers.contains_key("tier3") {
                    Some("tier3")
                } else {
                    self.tiers
                        .keys()
                        .find(|k| k.starts_with("tier-3-") || k.starts_with("tier3"))
                        .map(String::as_str)
                }
            })?;

        // 2. Find that tier in tiers map
        let tier = self.tiers.get(tier_name)?;

        // 3. Iterate through tier's models (format: tool/provider/model/thinking_budget)
        for model_spec_str in &tier.models {
            // Parse model spec to extract tool name
            let parts: Vec<&str> = model_spec_str.splitn(4, '/').collect();
            if parts.len() != 4 {
                continue; // Invalid format, skip
            }

            let tool_name = parts[0];

            // 4. Check if this tool is enabled
            if self.is_tool_enabled(tool_name) {
                return Some((tool_name.to_string(), model_spec_str.clone()));
            }
        }

        None
    }

    /// Save a user-level config template to `~/.config/cli-sub-agent/config.toml`.
    ///
    /// Creates the directory if needed. Returns the path written, or None
    /// if the config directory cannot be determined.
    pub fn save_user_config_template() -> Result<Option<PathBuf>> {
        let path = match Self::user_config_path() {
            Some(p) => p,
            None => return Ok(None),
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }
        std::fs::write(&path, Self::user_config_template())
            .with_context(|| format!("Failed to write user config template: {}", path.display()))?;
        Ok(Some(path))
    }

    /// Generate a commented template for user-level config.
    pub fn user_config_template() -> String {
        r#"# CSA User-Level Configuration
# Location: ~/.config/cli-sub-agent/config.toml
#
# This file provides default tiers, tool settings, and aliases
# that apply across all CSA projects unless overridden by
# project-level .csa/config.toml.

schema_version = 1

[resources]
# Minimum combined free memory (physical + swap) in MB.
min_free_memory_mb = 4096
# Kill child processes only when no streamed output appears for N seconds.
idle_timeout_seconds = 120

# Tool configuration defaults.
# [tools.codex]
# enabled = true

# [tools.gemini-cli]
# enabled = true

# Tier definitions.
# Uncomment and customize for your environment.
#
# [tiers.tier-1-quick]
# description = "Quick tasks: fast models"
# models = [
#     "gemini-cli/google/gemini-2.5-flash/low",
# ]
#
# [tiers.tier-2-standard]
# description = "Standard tasks: balanced models"
# models = [
#     "codex/openai/o3/medium",
#     "gemini-cli/google/gemini-2.5-pro/medium",
# ]
#
# [tiers.tier-3-heavy]
# description = "Complex tasks: strongest models"
# models = [
#     "claude-code/anthropic/claude-sonnet-4-5-20250929/high",
#     "codex/openai/o3/high",
# ]

# Tier mapping: task_type -> tier_name
# [tier_mapping]
# default = "tier-2-standard"
# quick = "tier-1-quick"
# complex = "tier-3-heavy"

# Aliases: shorthand -> full model spec
# [aliases]
# fast = "gemini-cli/google/gemini-2.5-flash/low"
# smart = "codex/openai/o3/high"
"#
        .to_string()
    }

    /// Resolve alias to model spec string.
    ///
    /// If input is an alias key, returns the resolved value.
    /// Otherwise, returns the input unchanged.
    pub fn resolve_alias(&self, input: &str) -> String {
        self.aliases
            .get(input)
            .cloned()
            .unwrap_or_else(|| input.to_string())
    }

    /// Check whether a full model spec string appears in any tier's models list.
    ///
    /// Performs exact string match against all tier model specs.
    pub fn is_model_spec_in_tiers(&self, spec: &str) -> bool {
        self.tiers
            .values()
            .any(|tier| tier.models.iter().any(|m| m == spec))
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
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "config_merge_tests.rs"]
mod merge_tests;
