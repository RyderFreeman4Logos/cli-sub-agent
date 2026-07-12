//! Global configuration for CLI Sub-Agent (`~/.config/cli-sub-agent/config.toml`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub use crate::global_env::ExecutionEnvOptions;
pub use crate::global_kv_cache::{
    DEFAULT_KV_CACHE_FREQUENT_POLL_SECS, DEFAULT_KV_CACHE_LONG_POLL_SECS, KvCacheConfig,
    KvCacheValueSource, LEGACY_SESSION_WAIT_FALLBACK_SECS, ProviderTtls, ResolvedKvCacheValue,
};
use crate::mcp::McpServerConfig;
use crate::memory::MemoryConfig;
pub use crate::tool_selection::ToolSelection;
use csa_core::types::ToolName;

const DEFAULT_MAX_CONCURRENT: u32 = 3;
pub const DEFAULT_CODEX_STATE_DIR: &str = "~/.codex";
pub const DEFAULT_CLAUDE_STATE_DIR: &str = "~/.claude";
/// Global configuration loaded from `~/.config/cli-sub-agent/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub preferences: PreferencesConfig,
    #[serde(default, skip_serializing_if = "GithubConfig::is_default")]
    pub github: GithubConfig,
    #[serde(default)]
    pub tools: HashMap<String, GlobalToolConfig>,
    #[serde(default)]
    pub review: ReviewConfig,
    #[serde(default)]
    pub debate: DebateConfig,
    #[serde(default)]
    pub fallback: FallbackConfig,
    /// Retry-loop stop policy shared by run/review/debate orchestration.
    #[serde(default)]
    pub retry: RetryConfig,
    /// Token budget defaults for issue-scoped sessions.
    #[serde(default)]
    pub budget: BudgetConfig,
    /// Global-only tier bypass policy.
    #[serde(default)]
    pub tier_policy: TierPolicyConfig,
    #[serde(default)]
    pub todo: TodoDisplayConfig,
    /// Memory system configuration.
    #[serde(default)]
    pub memory: MemoryConfig,
    /// Per-tool state directories exposed writable to sandboxed tool processes.
    #[serde(default = "default_tool_state_dirs")]
    pub tool_state_dirs: HashMap<String, PathBuf>,
    /// Global hook behavior settings.
    #[serde(default, skip_serializing_if = "GlobalHooksConfig::is_default")]
    pub hooks: GlobalHooksConfig,
    /// Global MCP servers; merged with project `.csa/mcp.toml` (project wins).
    #[serde(default)]
    pub mcp: GlobalMcpConfig,
    /// MCP hub unix socket for shared proxy mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_proxy_socket: Option<String>,
    /// Tool name aliases (`cx` → `codex`). Project-level wins.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tool_aliases: HashMap<String, String>,
    /// `csa run` behavior defaults; project config overrides through the merged project view.
    #[serde(default)]
    pub run: crate::config::RunConfig,
    /// Execution tuning; project-level `[execution]` overrides.
    #[serde(default)]
    pub execution: crate::config::ExecutionConfig,
    /// KV cache-aware polling defaults used by orchestration workflows.
    #[serde(default)]
    pub kv_cache: KvCacheConfig,
    /// Optional memory warning threshold for `csa session wait`.
    #[serde(default, skip_serializing_if = "SessionWaitConfig::is_default")]
    pub session_wait: SessionWaitConfig,
    /// Pre-flight repository integrity checks before session spawn.
    #[serde(default)]
    pub preflight: PreflightConfig,
    /// State directory size cap and monitoring configuration.
    #[serde(default, skip_serializing_if = "StateDirConfig::is_default")]
    pub state_dir: StateDirConfig,
    /// ACP transport overrides; project-level `[acp]` takes precedence.
    #[serde(default, skip_serializing_if = "crate::AcpConfig::is_default")]
    pub acp: crate::AcpConfig,
    /// Global filesystem sandbox defaults; project-level `[filesystem_sandbox]` overrides.
    #[serde(
        default,
        skip_serializing_if = "crate::config_filesystem_sandbox::FilesystemSandboxConfig::is_default"
    )]
    pub filesystem_sandbox: crate::config_filesystem_sandbox::FilesystemSandboxConfig,
    /// Experimental feature flags.
    #[serde(default)]
    pub experimental: ExperimentalConfig,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            defaults: DefaultsConfig::default(),
            preferences: PreferencesConfig::default(),
            github: GithubConfig::default(),
            tools: HashMap::new(),
            review: ReviewConfig::default(),
            debate: DebateConfig::default(),
            fallback: FallbackConfig::default(),
            retry: RetryConfig::default(),
            budget: BudgetConfig::default(),
            tier_policy: TierPolicyConfig::default(),
            todo: TodoDisplayConfig::default(),
            memory: MemoryConfig::default(),
            tool_state_dirs: default_tool_state_dirs(),
            hooks: GlobalHooksConfig::default(),
            mcp: GlobalMcpConfig::default(),
            mcp_proxy_socket: None,
            tool_aliases: HashMap::new(),
            run: crate::config::RunConfig::default(),
            execution: crate::config::ExecutionConfig::default(),
            kv_cache: KvCacheConfig::default(),
            session_wait: SessionWaitConfig::default(),
            preflight: PreflightConfig::default(),
            state_dir: StateDirConfig::default(),
            acp: crate::AcpConfig::default(),
            filesystem_sandbox: crate::config_filesystem_sandbox::FilesystemSandboxConfig::default(
            ),
            experimental: ExperimentalConfig::default(),
        }
    }
}

pub fn default_tool_state_dirs() -> HashMap<String, PathBuf> {
    HashMap::from([
        ("codex".to_string(), PathBuf::from(DEFAULT_CODEX_STATE_DIR)),
        (
            "claude".to_string(),
            PathBuf::from(DEFAULT_CLAUDE_STATE_DIR),
        ),
    ])
}

pub fn ensure_default_tool_state_dirs(tool_state_dirs: &mut HashMap<String, PathBuf>) {
    for (tool, path) in default_tool_state_dirs() {
        tool_state_dirs.entry(tool).or_insert(path);
    }
}

/// GitHub CLI authentication settings shared across workflows.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GithubConfig {
    /// Optional `GH_CONFIG_DIR` override for GitHub issue workflows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,
}

impl GithubConfig {
    pub fn is_default(&self) -> bool {
        self.config_dir.is_none()
    }
}

/// Global hook behavior settings (`[hooks]` in `~/.config/cli-sub-agent/config.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalHooksConfig {
    /// Auto-setup the pre-push review gate on each session start (rate-limited to once per hour).
    #[serde(default)]
    pub auto_setup_review_gate: bool,
}

impl GlobalHooksConfig {
    /// Returns true when all fields are at their defaults.
    pub fn is_default(&self) -> bool {
        !self.auto_setup_review_gate
    }
}

pub fn default_max_goal_loops() -> u32 {
    3
}

pub fn default_max_goal_tokens() -> u64 {
    500_000
}

pub fn default_task_pool_workers() -> u32 {
    1
}

pub fn default_retry_max_attempts() -> u8 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetryConfig {
    /// Maximum total attempts before retry loops stop.
    #[serde(default = "default_retry_max_attempts")]
    pub max_attempts: u8,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: default_retry_max_attempts(),
        }
    }
}

impl RetryConfig {
    pub fn resolved_max_attempts(&self) -> usize {
        usize::from(self.max_attempts.clamp(1, 20))
    }

    pub fn resolved_max_retries(&self) -> u8 {
        self.max_attempts.clamp(1, 20).saturating_sub(1)
    }

    pub fn is_default(&self) -> bool {
        self.max_attempts == default_retry_max_attempts()
    }
}

pub fn default_max_tokens_per_issue() -> u64 {
    5_000_000
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BudgetConfig {
    /// Maximum total tokens allocated to one issue/session chain.
    #[serde(default = "default_max_tokens_per_issue")]
    pub max_tokens_per_issue: u64,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_tokens_per_issue: default_max_tokens_per_issue(),
        }
    }
}

impl BudgetConfig {
    pub fn resolved_max_tokens_per_issue(&self) -> u64 {
        self.max_tokens_per_issue.max(1)
    }

    pub fn is_exhausted(&self, used_tokens: u64) -> bool {
        used_tokens >= self.resolved_max_tokens_per_issue()
    }

    pub fn is_default(&self) -> bool {
        self.max_tokens_per_issue == default_max_tokens_per_issue()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentalConfig {
    #[serde(default)]
    pub enable_prompt_caching: bool,
    #[serde(default = "default_max_goal_loops")]
    pub max_goal_loops: u32,
    #[serde(default = "default_max_goal_tokens")]
    pub max_goal_tokens: u64,
    #[serde(default = "default_task_pool_workers")]
    pub task_pool_workers: u32,
}

impl Default for ExperimentalConfig {
    fn default() -> Self {
        Self {
            enable_prompt_caching: false,
            max_goal_loops: default_max_goal_loops(),
            max_goal_tokens: default_max_goal_tokens(),
            task_pool_workers: default_task_pool_workers(),
        }
    }
}

/// Configuration for `csa session wait`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionWaitConfig {
    /// Optional process-tree RSS warning threshold in MB.
    ///
    /// `None` or `0` disables the sampler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_warn_mb: Option<u64>,
}

impl SessionWaitConfig {
    pub fn is_default(&self) -> bool {
        self.memory_warn_mb.is_none()
    }
}

/// Pre-flight checks that run before session creation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PreflightConfig {
    #[serde(
        default,
        skip_serializing_if = "AiConfigSymlinkCheckConfig::is_default"
    )]
    pub ai_config_symlink_check: AiConfigSymlinkCheckConfig,
}

/// Configuration for AI-config symlink integrity validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfigSymlinkCheckConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
    #[serde(default = "default_broken_as_error")]
    pub treat_broken_symlink_as_error: bool,
}

const fn default_broken_as_error() -> bool {
    true
}

impl Default for AiConfigSymlinkCheckConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            paths: None,
            treat_broken_symlink_as_error: default_broken_as_error(),
        }
    }
}

impl AiConfigSymlinkCheckConfig {
    pub fn is_default(&self) -> bool {
        !self.enabled && self.paths.is_none() && self.treat_broken_symlink_as_error
    }
}

/// State directory size monitoring and cap enforcement.
///
/// When `max_size_mb > 0`, CSA scans the state directory on preflight and
/// takes action when the size exceeds the cap (warn, error, or auto-gc).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDirConfig {
    /// Maximum size in MB. `0` (default) = unlimited.
    #[serde(default)]
    pub max_size_mb: u64,
    /// How often to recompute the size (seconds). `0` = every invocation.
    #[serde(default = "default_scan_interval_seconds")]
    pub scan_interval_seconds: u64,
    /// What to do when size exceeds `max_size_mb`.
    #[serde(default)]
    pub on_exceed: StateDirOnExceed,
}

const fn default_scan_interval_seconds() -> u64 {
    3600
}

impl Default for StateDirConfig {
    fn default() -> Self {
        Self {
            max_size_mb: 0,
            scan_interval_seconds: default_scan_interval_seconds(),
            on_exceed: StateDirOnExceed::default(),
        }
    }
}

impl StateDirConfig {
    pub fn is_default(&self) -> bool {
        self.max_size_mb == 0
            && self.scan_interval_seconds == default_scan_interval_seconds()
            && self.on_exceed == StateDirOnExceed::Warn
    }
}

/// Action taken when the state directory exceeds `max_size_mb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateDirOnExceed {
    /// Inject a warning preamble into the spawned session (non-blocking).
    #[default]
    Warn,
    /// Refuse to spawn the session with a clear error.
    Error,
    /// Run `csa gc` before spawning (Phase 3 — currently falls back to warn).
    AutoGc,
}

/// User preferences for tool selection and routing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PreferencesConfig {
    /// Default exact model selector for `csa run` when no model-selecting CLI
    /// flag is present.
    ///
    /// Format matches `csa run --model-spec`: `tool/provider/model/thinking_budget`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_writer_spec: Option<String>,
    /// Tool priority order for auto-selection. First = most preferred.
    ///
    /// Affects: heterogeneous candidate ordering, reviewer allocation,
    /// any-available fallback. Does NOT affect explicit `--tool` overrides
    /// or tier model declaration order.
    ///
    /// Tools not listed are appended in their default order.
    /// Empty list (default) preserves existing behavior.
    #[serde(default)]
    pub tool_priority: Vec<String>,
}

/// Global policy for exact-model and force bypasses when project tiers exist.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TierPolicyConfig {
    /// Allow emergency bypasses such as `--model-spec`, `--force`, and
    /// `--force-ignore-tier-setting` even when `[tiers]` are configured.
    ///
    /// This is global-only; project `.csa/config.toml` must not grant this.
    #[serde(default)]
    pub allow_force_bypass: bool,
}

#[path = "global_review.rs"]
mod review;
pub use review::*;

/// Display configuration for `csa todo` subcommands.
///
/// When set, output is piped through the specified external command.
/// Falls back to plain `print!()` when the command is absent or stdout is not a terminal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TodoDisplayConfig {
    /// Command to pipe `csa todo show` output through (e.g., `"bat -l md"`).
    #[serde(default)]
    pub show_command: Option<String>,
    /// Command to pipe `csa todo diff` output through (e.g., `"delta"`).
    #[serde(default)]
    pub diff_command: Option<String>,
}

/// Global MCP server configuration.
///
/// Servers listed here are injected into every spawned tool session.
/// Project-level `.csa/mcp.toml` servers override global ones with the same name.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalMcpConfig {
    /// MCP servers available to all tool sessions.
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

/// Returns the heterogeneous counterpart tool for model-diversity enforcement.
///
/// - `claude-code` → `codex`
/// - `codex` → `claude-code`
/// - Anything else → `None`
pub fn heterogeneous_counterpart(tool: &str) -> Option<&'static str> {
    match tool {
        "claude-code" => Some("codex"),
        "codex" => Some("claude-code"),
        _ => None,
    }
}

/// Select a tool from a different model family than the given tool.
/// Returns None if no heterogeneous tool is available.
pub fn select_heterogeneous_tool(
    parent_tool: &ToolName,
    available_tools: &[ToolName],
) -> Option<ToolName> {
    let parent_family = parent_tool.model_family();
    available_tools
        .iter()
        .find(|t| t.model_family() != parent_family)
        .copied()
}

/// Returns all currently supported tool names as a static slice.
pub fn all_known_tools() -> &'static [ToolName] {
    &[
        ToolName::Opencode,
        ToolName::Codex,
        ToolName::ClaudeCode,
        ToolName::OpenaiCompat,
        ToolName::Hermes,
        ToolName::AntigravityCli,
    ]
}

/// Returns tools eligible for automatic routing and general fallback.
pub fn routing_candidate_tools() -> &'static [ToolName] {
    csa_core::types::ROUTING_CANDIDATE_TOOLS
}

/// Sort tools by a priority list. Listed tools appear first (in priority order).
/// Unlisted tools retain their original relative order, appended after listed ones.
///
/// Returns the input unchanged when `priority` is empty (backward compatible).
pub fn sort_tools_by_priority(tools: &[ToolName], priority: &[String]) -> Vec<ToolName> {
    if priority.is_empty() {
        return tools.to_vec();
    }
    let mut result = tools.to_vec();
    result.sort_by_key(|tool| {
        priority
            .iter()
            .position(|p| p == tool.as_str())
            .unwrap_or(priority.len())
    });
    result
}

/// Resolve effective tool priority: project-level overrides global when present.
pub fn effective_tool_priority<'a>(
    project_config: Option<&'a crate::ProjectConfig>,
    global_config: &'a GlobalConfig,
) -> &'a [String] {
    project_config
        .and_then(|p| p.preferences.as_ref())
        .map(|p| p.tool_priority.as_slice())
        .filter(|p| !p.is_empty())
        .unwrap_or(&global_config.preferences.tool_priority)
}

/// Resolve the default exact writer model spec for `csa run`.
///
/// Project-level preferences override global preferences.
pub fn effective_primary_writer_spec<'a>(
    project_config: Option<&'a crate::ProjectConfig>,
    global_config: &'a GlobalConfig,
) -> Option<&'a str> {
    project_config
        .and_then(|p| p.preferences.as_ref())
        .and_then(|p| p.primary_writer_spec.as_deref())
        .or(global_config.preferences.primary_writer_spec.as_deref())
}

/// Sort tools using effective priority from project (if set) or global config.
pub fn sort_tools_by_effective_priority(
    tools: &[ToolName],
    project_config: Option<&crate::ProjectConfig>,
    global_config: &GlobalConfig,
) -> Vec<ToolName> {
    sort_tools_by_priority(
        tools,
        effective_tool_priority(project_config, global_config),
    )
}

/// Global defaults section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultsConfig {
    /// Default maximum concurrent instances per tool (default: 3).
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u32,
    /// Default parent tool context when auto-detection fails.
    #[serde(default)]
    pub tool: Option<String>,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            tool: None,
        }
    }
}

/// Per-tool global configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalToolConfig {
    /// Maximum concurrent instances for this tool. None = use defaults.
    #[serde(default)]
    pub max_concurrent: Option<u32>,
    /// Environment variables injected into child processes for this tool.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Per-tool memory limit override (MB). Takes precedence over project/global resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_max_mb: Option<u64>,
    /// Per-tool swap limit override (MB). Takes precedence over project/global resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_swap_max_mb: Option<u64>,
    /// Lock thinking budget for this tool. When set, any CLI `--thinking` or
    /// `--model-spec` thinking override is silently replaced with this value.
    /// Accepts: low, medium, high, xhigh, max, or a numeric token count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_lock: Option<String>,
    /// API key for fallback authentication where supported by the provider.
    /// NOT injected into env by default — only used as a last resort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Legacy no-op: retained for backward-compatible config deserialization.
    /// Defaults to true so startup-time MCP degradation is non-fatal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_degraded_mcp: Option<bool>,
    /// Codex-only: enable fast mode (2× cost, faster output).
    /// Equivalent to `--fast-but-more-cost` CLI flag; applies to all codex
    /// sessions when true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_mode: Option<bool>,
}

fn default_max_concurrent() -> u32 {
    DEFAULT_MAX_CONCURRENT
}

#[cfg(test)]
#[path = "global_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "global_tests_github.rs"]
mod tests_github;
#[cfg(test)]
#[path = "global_tests_heterogeneous.rs"]
mod tests_heterogeneous;
#[cfg(test)]
#[path = "global_tests_priority.rs"]
mod tests_priority;
#[cfg(test)]
#[path = "global_tests_review_batch.rs"]
mod tests_review_batch;
#[cfg(test)]
#[path = "global_tests_state_dir.rs"]
mod tests_state_dir;
