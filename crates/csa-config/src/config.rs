use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::acp::AcpConfig;
use crate::config_filesystem_sandbox::FilesystemSandboxConfig;
use crate::config_merge::{
    enforce_global_tool_disables, merge_toml_values, reject_project_convergence_completion_policy,
    reject_project_tier_policy, strip_review_project_only_from_global, warn_deprecated_keys,
};
use crate::config_raw::{
    prune_project_removed_refs, pruned_project_config_str, reject_removed_refs,
};
pub use crate::config_resources::ResourcesConfig;
use crate::global::{
    GithubConfig, PreferencesConfig, PreflightConfig, ReviewConfig, SessionWaitConfig,
    default_tool_state_dirs, ensure_default_tool_state_dirs,
};
use crate::memory::MemoryConfig;
use crate::paths;

mod captured;

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

/// Model selection strategy within a tier.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TierStrategy {
    /// Always try the first eligible model; advance only on quota/error.
    #[default]
    Priority,
    /// Cycle through models in order (round-robin).
    RoundRobin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierConfig {
    pub description: String,
    pub models: Vec<String>,
    /// Model selection strategy: `priority` (default) or `round-robin`.
    #[serde(default, skip_serializing_if = "is_default_strategy")]
    pub strategy: TierStrategy,
    /// Optional token budget allocated for sessions using this tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    /// Optional maximum number of execution turns for this tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
}

fn is_default_strategy(s: &TierStrategy) -> bool {
    *s == TierStrategy::Priority
}

fn numeric_tier_prefix(selector: &str) -> Option<String> {
    let suffix = selector.strip_prefix("tier")?;
    let digits = suffix.strip_prefix('-').unwrap_or(suffix);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("tier-{digits}"))
}

/// Current schema version for config.toml
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub project: ProjectMeta,
    #[serde(default, skip_serializing_if = "ResourcesConfig::is_default")]
    pub resources: ResourcesConfig,
    /// ACP transport behavior overrides.
    #[serde(default, skip_serializing_if = "AcpConfig::is_default")]
    pub acp: AcpConfig,
    /// Session-level behavior toggles.
    #[serde(default, skip_serializing_if = "SessionConfig::is_default")]
    pub session: SessionConfig,
    /// Memory system configuration.
    #[serde(default, skip_serializing_if = "MemoryConfig::is_default")]
    pub memory: MemoryConfig,
    /// Per-tool state directories exposed writable to sandboxed tool processes.
    ///
    /// Example:
    /// ```toml
    /// [tool_state_dirs]
    /// codex = "~/.codex"
    /// claude = "~/.claude"
    /// ```
    #[serde(
        default = "default_tool_state_dirs",
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub tool_state_dirs: HashMap<String, PathBuf>,
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
    /// Tool name aliases: maps short names to canonical tool names.
    ///
    /// Example: `cx = "codex"`, `cc = "claude-code"`.
    /// Built-in aliases (`claude` → `claude-code`)
    /// are always available without configuration.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tool_aliases: HashMap<String, String>,
    /// Optional per-project tool priority override.
    /// When set, overrides the global `[preferences].tool_priority`.
    #[serde(default)]
    pub preferences: Option<PreferencesConfig>,
    /// Optional per-project GitHub auth override for issue workflows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<GithubConfig>,
    /// Project-level hook overrides for pre/post run commands.
    ///
    /// When set, `.csa/config.toml` hooks take PRIORITY over `hooks.toml`
    /// for PreRun/PostRun events. The commands specified here are injected
    /// as runtime overrides into the hook loading pipeline.
    #[serde(default, skip_serializing_if = "HooksSection::is_default")]
    pub hooks: HooksSection,
    /// `csa run` behavior toggles and post-exec verification.
    #[serde(default)]
    pub run: RunConfig,
    /// Execution tuning knobs (timeout floors, etc.).
    #[serde(default, skip_serializing_if = "ExecutionConfig::is_default")]
    pub execution: ExecutionConfig,
    #[serde(default)]
    pub session_wait: Option<SessionWaitConfig>,
    #[serde(default, skip_serializing_if = "preflight_is_default")]
    pub preflight: PreflightConfig,
    #[serde(default, skip_serializing_if = "VcsConfig::is_default")]
    pub vcs: VcsConfig,
    #[serde(default, skip_serializing_if = "FilesystemSandboxConfig::is_default")]
    pub filesystem_sandbox: FilesystemSandboxConfig,
}

fn preflight_is_default(config: &PreflightConfig) -> bool {
    config.ai_config_symlink_check.is_default()
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

pub use super::config_session::{
    DEFAULT_COOLDOWN_SECS, DEFAULT_FORK_PREFIX_BUDGET_TOKENS,
    DEFAULT_RESULT_REPORT_SPILL_THRESHOLD_BYTES, ExecutionConfig, FORK_PREFIX_BUDGET_MAX_TOKENS,
    FORK_PREFIX_BUDGET_MIN_TOKENS, HooksSection, PostExecGateConfig, RunConfig, SessionConfig,
    SnapshotTrigger, VcsConfig,
};
pub use super::config_tool::{
    ToolConfig, ToolFilesystemSandboxConfig, ToolRestrictions, TransportKind,
};

impl ProjectConfig {
    /// Return a copy suitable for user-facing display/logging.
    ///
    /// Sensitive fields (e.g. API keys) are masked.
    pub fn redacted_for_display(&self) -> Self {
        let mut redacted = self.clone();
        redacted.memory.llm = redacted.memory.llm.redacted_for_display();
        for tool_cfg in redacted.tools.values_mut() {
            if tool_cfg.api_key.is_some() {
                tool_cfg.api_key = Some("***REDACTED***".to_string());
            }
        }
        redacted
    }

    /// Load config with fallback chain:
    ///
    /// 1. If both `.csa/config.toml` (project) and user config exist, deep-merge
    ///    them with project settings overriding user settings.
    /// 2. If only project config exists, use it directly.
    /// 3. If only user config exists, use it as fallback.
    /// 4. If neither exists, return None.
    pub fn load(project_root: &Path) -> Result<Option<Self>> {
        let project_path = project_root.join(".csa").join("config.toml");
        let user_path = Self::user_config_path();
        Self::load_with_paths(user_path.as_deref(), &project_path)
    }

    /// Load only the project-level `.csa/config.toml`, skipping user/global fallback.
    ///
    /// Missing project config returns `Ok(None)`.
    pub fn load_project_only(project_root: &Path) -> Result<Option<Self>> {
        let project_path = project_root.join(".csa").join("config.toml");
        Self::load_with_paths(None, &project_path)
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
            (false, true) => Self::load_project_from_path(project_path),
            (true, true) => {
                // Safety: user_exists guarantees user_path is Some
                Self::load_merged(user_path.unwrap(), project_path)
            }
        }
    }

    fn load_from_path(path: &Path) -> Result<Option<Self>> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config: {}", path.display()))?;
        Self::parse_user_contents(path, &content)
    }

    fn load_project_from_path(path: &Path) -> Result<Option<Self>> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config: {}", path.display()))?;
        Self::parse_project_contents(path, &content)
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
        Self::parse_merged_contents(base_path, &base_str, overlay_path, &overlay_str)
    }

    fn sanitize_filesystem_sandbox(&mut self) {
        ensure_default_tool_state_dirs(&mut self.tool_state_dirs);
        self.filesystem_sandbox.sanitize_legacy_xdg_runtime_root();
        for (tool, config) in &mut self.tools {
            let Some(sandbox) = &mut config.filesystem_sandbox else {
                continue;
            };
            let Some(paths) = sandbox.writable_paths.as_mut() else {
                continue;
            };
            let context = format!("tools.{tool}.filesystem_sandbox.writable_paths");
            crate::config_filesystem_sandbox::sanitize_legacy_xdg_runtime_root_paths(
                paths, &context,
            );
        }
    }

    /// Path to user-level config for reads.
    ///
    /// Prefers `~/.config/cli-sub-agent/config.toml`, and falls back to
    /// `~/.config/csa/config.toml` when the canonical `~/.config/cli-sub-agent/config.toml` path is absent.
    ///
    /// Returns None if the config directory cannot be determined
    /// (e.g., no HOME in containers).
    pub fn user_config_path() -> Option<PathBuf> {
        paths::config_dir().map(|dir| dir.join("config.toml"))
    }

    /// Resolve `[session_wait].memory_warn_mb` from the merged user/project config view.
    ///
    /// `0` and invalid values are treated as disabled.
    pub fn resolve_session_wait_memory_warn_mb(project_root: &Path) -> Option<u64> {
        let project_path = project_root.join(".csa").join("config.toml");
        let user_path = Self::user_config_path();
        Self::resolve_session_wait_memory_warn_mb_with_paths(user_path.as_deref(), &project_path)
    }

    pub(crate) fn resolve_session_wait_memory_warn_mb_with_paths(
        user_path: Option<&Path>,
        project_path: &Path,
    ) -> Option<u64> {
        let project_raw = read_optional_toml(project_path, "project");
        let user_raw = user_path.and_then(|path| read_optional_toml(path, "user"));
        let merged = match (user_raw, project_raw) {
            (Some(base), Some(overlay)) => merge_toml_values(base, overlay),
            (Some(base), None) => base,
            (None, Some(overlay)) => overlay,
            (None, None) => return None,
        };

        parse_session_wait_memory_warn_mb(&merged)
    }

    fn user_config_write_path() -> Option<PathBuf> {
        paths::config_dir_write().map(|dir| dir.join("config.toml"))
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

    /// Get the thinking budget lock for a tool from project config.
    pub fn thinking_lock(&self, tool: &str) -> Option<&str> {
        self.tools
            .get(tool)
            .and_then(|t| t.thinking_lock.as_deref())
    }

    /// Enforce that a tool is enabled in user configuration.
    ///
    /// Returns `Ok(())` when the tool is enabled or not configured (defaults to enabled).
    /// Returns an error with a prompt-injection-aware message when `enabled = false`
    /// is set explicitly in config.
    ///
    /// The `force_override` parameter allows callers to bypass the check when the
    /// user has explicitly passed `--force-override-user-config`.
    pub fn enforce_tool_enabled(&self, tool: &str, force_override: bool) -> anyhow::Result<()> {
        if force_override {
            return Ok(());
        }
        if !self.is_tool_enabled(tool) {
            // Build a list of other enabled tools to guide the caller to alternatives.
            let alternatives: Vec<&str> = crate::global::all_known_tools()
                .iter()
                .map(|t| t.as_str())
                .filter(|&name| name != tool && self.is_tool_enabled(name))
                .collect();
            let alternatives_hint = if alternatives.is_empty() {
                String::new()
            } else {
                format!("\nCurrently enabled tools: {}.", alternatives.join(", "))
            };
            anyhow::bail!(
                "Error: tool '{tool}' is disabled in user configuration.\n\
                 The user may have temporarily disabled this tool. Respect their preference.\n\
                 To override, use --force-override-user-config (not recommended unless\n\
                 the user explicitly requested this specific tool).{alternatives_hint}"
            );
        }
        Ok(())
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

    /// Resolve a tier selector (direct name, `tier_mapping` alias, numeric shorthand, or
    /// unambiguous prefix) to canonical tier name.
    ///
    /// Priority: exact name > alias > numeric shorthand > unique prefix. No tier3 fallback.
    pub fn resolve_tier_selector(&self, selector: &str) -> Option<String> {
        // Reject empty/whitespace-only selectors early — prevents prefix matching
        // from silently resolving "" to the sole tier in single-tier configs.
        if selector.trim().is_empty() {
            return None;
        }
        // 1. Exact tier name match
        if self.tiers.contains_key(selector) {
            return Some(selector.to_string());
        }
        // 2. Alias lookup via tier_mapping
        if let Some(mapped) = self.tier_mapping.get(selector)
            && self.tiers.contains_key(mapped.as_str())
        {
            return Some(mapped.clone());
        }
        // 3. Numeric shorthand: tier4 and tier-4 match the first tier whose name starts
        // with tier-4 (for example, tier-4-critical). Sort for deterministic HashMap order.
        if let Some(prefix) = numeric_tier_prefix(selector) {
            let mut prefix_matches: Vec<&String> = self
                .tiers
                .keys()
                .filter(|name| name.starts_with(&prefix))
                .collect();
            prefix_matches.sort();
            if let Some(match_name) = prefix_matches.first() {
                return Some((*match_name).clone());
            }
        }
        // 4. Unambiguous prefix match: selector must match exactly one tier name
        let prefix_matches: Vec<&String> = self
            .tiers
            .keys()
            .filter(|name| name.starts_with(selector))
            .collect();
        if prefix_matches.len() == 1 {
            return Some(prefix_matches[0].clone());
        }
        None
    }

    // Compound tier-tool parsing, tier suggestion, and alias formatting
    // are in config_tier_helpers.rs to stay under the 800-line monolith gate.
    // Methods: try_parse_compound_tier_tool, suggest_tier, format_tier_aliases

    /// Resolve tier-based tool selection for a given task type.
    ///
    /// Returns (tool_name, model_spec_string) for the first enabled tool in the tier.
    /// Falls back to tier3 if task_type not found in tier_mapping.
    /// Returns None if no enabled tools found.
    pub fn resolve_tier_tool(&self, task_type: &str) -> Option<(String, String)> {
        self.resolve_tier_tool_filtered(task_type, false)
    }

    /// Resolve tier-based tool selection with write restriction filtering.
    ///
    /// When `needs_edit` is true, skips tools that are not fully write-capable
    /// (i.e., tools where `allow_edit_existing_files` or `allow_write_new_files` is false).
    pub fn resolve_tier_tool_filtered(
        &self,
        task_type: &str,
        needs_edit: bool,
    ) -> Option<(String, String)> {
        let tier_name = self.resolve_tier_name_for_task(task_type)?;

        let tier = self.tiers.get(tier_name)?;

        for model_spec_str in &tier.models {
            let parts: Vec<&str> = model_spec_str.splitn(4, '/').collect();
            if parts.len() != 4 {
                continue;
            }
            let tool_name = parts[0];
            if !self.is_tool_enabled(tool_name) {
                continue;
            }
            if needs_edit && !self.is_tool_write_capable(tool_name) {
                continue;
            }
            return Some((tool_name.to_string(), model_spec_str.clone()));
        }

        None
    }

    /// Save a user-level config template to `~/.config/cli-sub-agent/config.toml`.
    ///
    /// Creates the directory if needed. Returns the path written, or None
    /// if the config directory cannot be determined.
    pub fn save_user_config_template() -> Result<Option<PathBuf>> {
        let path = match Self::user_config_write_path() {
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
        r#"# CSA User-Level Configuration (~/.config/cli-sub-agent/config.toml)
schema_version = 1
[session]
transcript_enabled = false
transcript_redaction = true
# result_report_spill_threshold_bytes = 10240
# require_commit_on_mutation = true
[run]
# writer_must_commit = false
[resources]
min_free_memory_mb = 4096
idle_timeout_seconds = 250
liveness_dead_seconds = 600
slot_wait_timeout_seconds = 250
stdin_write_timeout_seconds = 30
termination_grace_period_seconds = 5
[kv_cache]
frequent_poll_seconds = 60
# General long-poll TTL; not a providerless `csa session wait` fallback.
default_ttl_seconds = 240
# `csa session wait` requires a positive-TTL key passed via --model-provider.
# The configured value is used exactly as the wait cap; no source fallback or maximum applies.
[kv_cache.provider_ttls]
claude = 3300
openai = 1700
glm = 540
xai = 1700
other = 270
[gc]
transcript_max_age_days = 30
transcript_max_size_mb = 500
reap_runtime_dirs = true
[acp]
init_timeout_seconds = 120
# [tools.codex]
# enabled = true
# codex_auto_trust = false
# tmux_mode = false
# [tiers.tier-1-quick]
# description = "Quick tasks: fast models"
# models = ["codex/openai/gpt-5.4/low"]
# [tiers.tier-2-standard]
# description = "Standard tasks: balanced models"
# models = ["codex/openai/gpt-5.4/medium", "claude-code/anthropic/claude-sonnet-4-5-20250929/medium"]
# [tiers.tier-3-heavy]
# description = "Complex tasks: strongest models"
# models = ["claude-code/anthropic/claude-sonnet-4-5-20250929/high", "codex/openai/gpt-5.5/high"]
# [tier_mapping]
# default = "tier-2-standard"
# quick = "tier-1-quick"
# complex = "tier-3-heavy"
# [aliases]
# fast = "codex/openai/gpt-5.4/low"
# smart = "codex/openai/gpt-5.5/high"
# [tool_aliases]
# cc = "claude-code"
# [hooks]
# pre_run = "cargo fmt --all"
# post_run = "cargo fmt --all"
# timeout_secs = 60
# [execution]
# min_timeout_seconds = 1800
"#
        .to_string()
    }

    /// Resolve alias to model spec string.
    ///
    /// If input is an alias key, returns the resolved value.
    pub fn resolve_alias(&self, input: &str) -> String {
        self.aliases
            .get(input)
            .cloned()
            .unwrap_or_else(|| input.to_string())
    }
}

pub(crate) fn read_optional_toml(path: &Path, source: &str) -> Option<toml::Value> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                source,
                error = %err,
                "Failed to read config while resolving layered project settings"
            );
            return None;
        }
    };

    match toml::from_str::<toml::Value>(&content) {
        Ok(raw) => Some(raw),
        Err(err) => {
            tracing::error!(
                path = %path.display(),
                source,
                error = %err,
                "Failed to parse config while resolving layered project settings"
            );
            None
        }
    }
}

fn parse_session_wait_memory_warn_mb(raw: &toml::Value) -> Option<u64> {
    let value = raw
        .get("session_wait")
        .and_then(|session_wait| session_wait.get("memory_warn_mb"))?;
    let limit = value.as_integer()?;
    if limit <= 0 {
        return None;
    }
    u64::try_from(limit).ok()
}

#[cfg(test)]
#[path = "config_merge_tests.rs"]
mod merge_tests;
#[cfg(test)]
#[path = "config_merge_tests_tail.rs"]
mod merge_tests_tail;
#[cfg(test)]
#[path = "config_tests_preflight.rs"]
mod preflight_tests;
#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "config_tests_github.rs"]
mod tests_github;
#[cfg(test)]
#[path = "config_tests_removed_gemini.rs"]
mod tests_removed_gemini;
#[cfg(test)]
#[path = "config_tests_tail.rs"]
mod tests_tail;
#[cfg(test)]
#[path = "config_tests_tier_selector_legacy.rs"]
mod tier_selector_legacy_tests;
#[cfg(test)]
#[path = "config_tests_tier_selector.rs"]
mod tier_selector_tests;
#[cfg(test)]
#[path = "config_tests_tier.rs"]
mod tier_tests;
