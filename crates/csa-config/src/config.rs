use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::acp::AcpConfig;
use crate::config_filesystem_sandbox::FilesystemSandboxConfig;
use crate::config_merge::{
    enforce_global_tool_disables, merge_toml_values, strip_review_project_only_from_global,
    warn_deprecated_keys,
};
pub use crate::config_resources::ResourcesConfig;
use crate::global::{PreferencesConfig, ReviewConfig};
use crate::memory::MemoryConfig;
use crate::paths;
use csa_core::vcs::VcsKind;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Persist ACP transcript events to `output/acp-events.jsonl` when enabled.
    #[serde(default)]
    pub transcript_enabled: bool,
    /// Redact sensitive content before writing transcript events to disk.
    #[serde(default = "default_true")]
    pub transcript_redaction: bool,
    /// Inject structured output section markers into prompts.
    /// When enabled, agents are instructed to wrap output in
    /// `<!-- CSA:SECTION:<id> -->` delimiters for machine-readable parsing.
    #[serde(default = "default_true")]
    pub structured_output: bool,
    /// Maximum age (seconds) for a seed session to remain valid.
    /// Sessions older than this are not eligible as fork sources.
    #[serde(default = "default_seed_max_age_secs")]
    pub seed_max_age_secs: u64,
    /// Automatically fork from a warm seed session instead of cold starting.
    #[serde(default = "default_true")]
    pub auto_seed_fork: bool,
    /// Maximum number of seed sessions retained per tool×project combination.
    /// Oldest seeds beyond this limit are retired (LRU eviction).
    #[serde(default = "default_max_seed_sessions")]
    pub max_seed_sessions: u32,
    /// Fail `csa run` when the workspace is mutated without creating a commit.
    ///
    /// Fail-closed mode is disabled by default; mutation guard stays warning-only.
    #[serde(default)]
    pub require_commit_on_mutation: bool,
    /// Maximum spool file size in megabytes before rotation (default 32).
    #[serde(default)]
    pub spool_max_mb: Option<u32>,
    /// Keep rotated spool files for debugging (default true).
    #[serde(default)]
    pub spool_keep_rotated: Option<bool>,
}

fn default_seed_max_age_secs() -> u64 {
    86400 // 24 hours
}

fn default_max_seed_sessions() -> u32 {
    2
}

const DEFAULT_SPOOL_MAX_MB: u32 = 32;
const DEFAULT_SPOOL_KEEP_ROTATED: bool = true;

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            transcript_enabled: false,
            transcript_redaction: true,
            structured_output: true,
            seed_max_age_secs: default_seed_max_age_secs(),
            auto_seed_fork: true,
            max_seed_sessions: default_max_seed_sessions(),
            require_commit_on_mutation: false,
            spool_max_mb: None,
            spool_keep_rotated: None,
        }
    }
}

impl SessionConfig {
    pub fn is_default(&self) -> bool {
        !self.transcript_enabled
            && self.transcript_redaction
            && self.structured_output
            && self.seed_max_age_secs == default_seed_max_age_secs()
            && self.auto_seed_fork
            && self.max_seed_sessions == default_max_seed_sessions()
            && !self.require_commit_on_mutation
            && self.spool_max_mb.is_none()
            && self.spool_keep_rotated.is_none()
    }

    pub fn resolved_spool_max_mb(&self) -> u32 {
        self.spool_max_mb.unwrap_or(DEFAULT_SPOOL_MAX_MB)
    }

    pub fn resolved_spool_keep_rotated(&self) -> bool {
        self.spool_keep_rotated
            .unwrap_or(DEFAULT_SPOOL_KEEP_ROTATED)
    }
}

/// Project-level hook overrides (`[hooks]` in `.csa/config.toml`).
///
/// When set, these commands take PRIORITY over `hooks.toml` PreRun/PostRun
/// entries. They are injected as runtime overrides into the hook loading
/// pipeline, so they sit at the highest-priority layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksSection {
    /// Shell command to run before every `csa run`/`review`/`debate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_run: Option<String>,
    /// Shell command to run after every `csa run`/`review`/`debate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_run: Option<String>,
    /// Timeout (seconds) for hook commands (default: 60).
    #[serde(
        default = "default_hooks_timeout_secs",
        skip_serializing_if = "is_default_hooks_timeout"
    )]
    pub timeout_secs: u64,
}

const fn default_hooks_timeout_secs() -> u64 {
    60
}

fn is_default_hooks_timeout(val: &u64) -> bool {
    *val == default_hooks_timeout_secs()
}

impl Default for HooksSection {
    fn default() -> Self {
        Self {
            pre_run: None,
            post_run: None,
            timeout_secs: default_hooks_timeout_secs(),
        }
    }
}

impl HooksSection {
    /// Returns true when all fields are at their defaults (per rust/016 serde-default rule).
    pub fn is_default(&self) -> bool {
        self.pre_run.is_none()
            && self.post_run.is_none()
            && self.timeout_secs == default_hooks_timeout_secs()
    }
}

/// Execution tuning (`[execution]` in config).
///
/// Present in both project and global configs. Project values override global
/// during config merge (standard TOML deep-merge).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    /// Floor for the `--timeout` flag (seconds).
    ///
    /// Any `--timeout` value below this is rejected at the CLI validation layer.
    /// Default: 1800 (30 minutes). The previous hardcoded floor was 1200.
    #[serde(
        default = "default_min_timeout_seconds",
        skip_serializing_if = "is_default_min_timeout"
    )]
    pub min_timeout_seconds: u64,
    /// When enabled, automatically run `weave upgrade` before CSA command execution.
    /// Silent output, exponential backoff retry on failure (2 retries), error exit
    /// if all retries fail. Default: false (opt-in).
    #[serde(default)]
    pub auto_weave_upgrade: bool,
}

const fn default_min_timeout_seconds() -> u64 {
    1800
}

fn is_default_min_timeout(val: &u64) -> bool {
    *val == default_min_timeout_seconds()
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            min_timeout_seconds: default_min_timeout_seconds(),
            auto_weave_upgrade: false,
        }
    }
}

impl ExecutionConfig {
    /// Returns true when all fields are at their defaults (per rust/016 serde-default rule).
    pub fn is_default(&self) -> bool {
        self.min_timeout_seconds == default_min_timeout_seconds() && !self.auto_weave_upgrade
    }

    /// The compile-time default minimum timeout in seconds.
    pub const fn default_min_timeout() -> u64 {
        default_min_timeout_seconds()
    }
}

/// VCS backend configuration.
///
/// Controls which VCS backend CSA uses for the project.
/// When `backend` is `None`, auto-detection is used (`.jj/` → Jj, `.git` → Git).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VcsConfig {
    /// Explicit VCS backend override. `None` means auto-detect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<VcsKind>,
    /// Default backend for colocated repos (both `.jj` and `.git` present).
    /// Defaults to Git when not set, overriding auto-detect's jj preference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colocated_default: Option<VcsKind>,
}

impl VcsConfig {
    pub fn is_default(&self) -> bool {
        self.backend.is_none() && self.colocated_default.is_none()
    }
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
    /// ACP transport behavior overrides.
    #[serde(default, skip_serializing_if = "AcpConfig::is_default")]
    pub acp: AcpConfig,
    /// Session-level behavior toggles.
    #[serde(default, skip_serializing_if = "SessionConfig::is_default")]
    pub session: SessionConfig,
    /// Memory system configuration.
    #[serde(default, skip_serializing_if = "MemoryConfig::is_default")]
    pub memory: MemoryConfig,
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
    /// Example: `gem = "gemini-cli"`, `cc = "claude-code"`.
    /// Built-in aliases (`gemini` → `gemini-cli`, `claude` → `claude-code`)
    /// are always available without configuration.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tool_aliases: HashMap<String, String>,
    /// Optional per-project tool priority override.
    /// When set, overrides the global `[preferences].tool_priority`.
    #[serde(default)]
    pub preferences: Option<PreferencesConfig>,
    /// Project-level hook overrides for pre/post run commands.
    ///
    /// When set, `.csa/config.toml` hooks take PRIORITY over `hooks.toml`
    /// for PreRun/PostRun events. The commands specified here are injected
    /// as runtime overrides into the hook loading pipeline.
    #[serde(default, skip_serializing_if = "HooksSection::is_default")]
    pub hooks: HooksSection,
    /// Execution tuning knobs (timeout floors, etc.).
    #[serde(default, skip_serializing_if = "ExecutionConfig::is_default")]
    pub execution: ExecutionConfig,
    #[serde(default, skip_serializing_if = "VcsConfig::is_default")]
    pub vcs: VcsConfig,
    #[serde(default, skip_serializing_if = "FilesystemSandboxConfig::is_default")]
    pub filesystem_sandbox: FilesystemSandboxConfig,
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

use super::config_tool::default_true;
pub use super::config_tool::{ToolConfig, ToolRestrictions};

impl ProjectConfig {
    /// Return a copy suitable for user-facing display/logging.
    ///
    /// Sensitive fields (e.g. API keys) are masked.
    pub fn redacted_for_display(&self) -> Self {
        let mut redacted = self.clone();
        redacted.memory.llm = redacted.memory.llm.redacted_for_display();
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
        if let Ok(raw) = toml::from_str::<toml::Value>(&content) {
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

        // Strip project-only review keys from global config before merge.
        // These fields are meaningful only in project config.
        let mut base_for_merge = base_val.clone();
        strip_review_project_only_from_global(&mut base_for_merge);

        let mut merged = merge_toml_values(base_for_merge, overlay_val);
        // Set schema_version to max of both sources (only when at least one is explicit)
        if let Some(max_ver) = match (base_schema, overlay_schema) {
            (Some(b), Some(o)) => Some(b.max(o)),
            (Some(v), None) | (None, Some(v)) => Some(v),
            (None, None) => None,
        } && let toml::Value::Table(ref mut table) = merged
        {
            table.insert("schema_version".to_string(), toml::Value::Integer(max_ver));
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
            anyhow::bail!(
                "Error: tool '{tool}' is disabled in user configuration.\n\
                 The user may have temporarily disabled this tool. Respect their preference.\n\
                 To override, use --force-override-user-config (not recommended unless\n\
                 the user explicitly requested this specific tool)."
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

    /// Resolve a tier selector (direct name or `tier_mapping` alias) to canonical tier name.
    /// Direct tier names take priority. No tier3 fallback.
    pub fn resolve_tier_selector(&self, selector: &str) -> Option<String> {
        if self.tiers.contains_key(selector) {
            return Some(selector.to_string());
        }
        self.tier_mapping
            .get(selector)
            .filter(|t| self.tiers.contains_key(t.as_str()))
            .cloned()
    }

    /// Format tier aliases for error messages (empty string if no mappings).
    pub fn format_tier_aliases(&self) -> String {
        if self.tier_mapping.is_empty() {
            return String::new();
        }
        let mut aliases: Vec<String> = self
            .tier_mapping
            .iter()
            .map(|(k, v)| format!("{k} \u{2192} {v}"))
            .collect();
        aliases.sort();
        format!("\nAvailable tier aliases: [{}]", aliases.join(", "))
    }

    /// Resolve tier-based tool selection for a given task type.
    ///
    /// Returns (tool_name, model_spec_string) for the first enabled tool in the tier.
    /// Falls back to tier3 if task_type not found in tier_mapping.
    /// Returns None if no enabled tools found.
    pub fn resolve_tier_tool(&self, task_type: &str) -> Option<(String, String)> {
        self.resolve_tier_tool_filtered(task_type, false)
    }

    /// Resolve tier-based tool selection with edit restriction filtering.
    ///
    /// When `needs_edit` is true, skips tools whose
    /// `restrictions.allow_edit_existing_files` is `false`.
    pub fn resolve_tier_tool_filtered(
        &self,
        task_type: &str,
        needs_edit: bool,
    ) -> Option<(String, String)> {
        let tier_name = self
            .tier_mapping
            .get(task_type)
            .map(String::as_str)
            .or_else(|| {
                if self.tiers.contains_key("tier3") {
                    Some("tier3")
                } else {
                    self.tiers
                        .keys()
                        .find(|k| k.starts_with("tier-3-") || k.starts_with("tier3"))
                        .map(String::as_str)
                }
            })?;

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
            if needs_edit && !self.can_tool_edit_existing(tool_name) {
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
# require_commit_on_mutation = true
[resources]
min_free_memory_mb = 4096
idle_timeout_seconds = 300
liveness_dead_seconds = 600
slot_wait_timeout_seconds = 300
stdin_write_timeout_seconds = 30
termination_grace_period_seconds = 5
[gc]
transcript_max_age_days = 30
transcript_max_size_mb = 500
[acp]
init_timeout_seconds = 120
# [tools.codex]
# enabled = true
# codex_auto_trust = false
# [tools.gemini-cli]
# enabled = true
# [tiers.tier-1-quick]
# description = "Quick tasks: fast models"
# models = ["gemini-cli/google/gemini-2.5-flash/low"]
# [tiers.tier-2-standard]
# description = "Standard tasks: balanced models"
# models = ["codex/openai/o3/medium", "gemini-cli/google/gemini-2.5-pro/medium"]
# [tiers.tier-3-heavy]
# description = "Complex tasks: strongest models"
# models = ["claude-code/anthropic/claude-sonnet-4-5-20250929/high", "codex/openai/o3/high"]
# [tier_mapping]
# default = "tier-2-standard"
# quick = "tier-1-quick"
# complex = "tier-3-heavy"
# [aliases]
# fast = "gemini-cli/google/gemini-2.5-flash/low"
# smart = "codex/openai/o3/high"
# [tool_aliases]
# gem = "gemini-cli"
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
    /// Otherwise, returns the input unchanged.
    pub fn resolve_alias(&self, input: &str) -> String {
        self.aliases
            .get(input)
            .cloned()
            .unwrap_or_else(|| input.to_string())
    }
}

#[cfg(test)]
#[path = "config_merge_tests.rs"]
mod merge_tests;
#[cfg(test)]
#[path = "config_merge_tests_tail.rs"]
mod merge_tests_tail;
#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "config_tests_tail.rs"]
mod tests_tail;
#[cfg(test)]
#[path = "config_tests_tier.rs"]
mod tier_tests;
