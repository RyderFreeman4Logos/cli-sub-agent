//! Global configuration for CLI Sub-Agent (`~/.config/cli-sub-agent/config.toml`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use crate::global_env::ExecutionEnvOptions;
pub use crate::global_kv_cache::{
    DEFAULT_KV_CACHE_FREQUENT_POLL_SECS, DEFAULT_KV_CACHE_LONG_POLL_SECS, KvCacheConfig,
    KvCacheValueSource, LEGACY_SESSION_WAIT_FALLBACK_SECS, ResolvedKvCacheValue,
};
use crate::mcp::McpServerConfig;
use crate::memory::MemoryConfig;
pub use crate::tool_selection::ToolSelection;
use csa_core::types::ToolName;

const DEFAULT_MAX_CONCURRENT: u32 = 3;

/// Global configuration loaded from `~/.config/cli-sub-agent/config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalConfig {
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub preferences: PreferencesConfig,
    #[serde(default)]
    pub tools: HashMap<String, GlobalToolConfig>,
    #[serde(default)]
    pub review: ReviewConfig,
    #[serde(default)]
    pub debate: DebateConfig,
    #[serde(default)]
    pub fallback: FallbackConfig,
    #[serde(default)]
    pub todo: TodoDisplayConfig,
    /// Memory system configuration.
    #[serde(default)]
    pub memory: MemoryConfig,
    /// Global MCP servers; merged with project `.csa/mcp.toml` (project wins).
    #[serde(default)]
    pub mcp: GlobalMcpConfig,
    /// MCP hub unix socket for shared proxy mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_proxy_socket: Option<String>,
    /// Tool name aliases (`gem` → `gemini-cli`). Project-level wins.
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

/// Configuration for the code review workflow.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GateMode {
    /// Log review results only; never block execution.
    #[default]
    Monitor,
    /// Block only on Critical and High severity findings.
    CriticalOnly,
    /// Block on Critical, High, and Medium severity findings.
    Full,
}

/// A single step in the pre-review quality gate pipeline (L1–L3).
/// Steps execute sequentially in ascending level order; aborts on first failure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateStep {
    pub name: String,
    pub command: String,
    /// Verification level: 1 = lint, 2 = type/boundary, 3 = test.
    #[serde(default = "default_gate_level")]
    pub level: u8,
}

const fn default_gate_level() -> u8 {
    1
}

/// Configuration for the code review workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewConfig {
    /// Review tool selection.
    ///
    /// Accepts a single tool name (`"codex"`), `"auto"` for heterogeneous
    /// auto-selection, or an array of tool names (`["codex", "gemini-cli"]`)
    /// as a whitelist for auto-selection. An empty array is equivalent to `"auto"`.
    #[serde(default)]
    pub tool: ToolSelection,
    /// Review enforcement level for quality gates.
    #[serde(default)]
    pub gate_mode: GateMode,
    /// Run cumulative review at most once per N commits.
    ///
    /// `1` preserves the existing behavior: review every round.
    /// Values `>= 2` allow intermediate workflows to skip cumulative review
    /// until at least N new commits have landed since the last passed
    /// `main...HEAD` cumulative review on the current branch.
    #[serde(
        default = "default_review_batch_commits",
        skip_serializing_if = "is_default_review_batch_commits"
    )]
    pub batch_commits: u32,
    /// Tier-based tool selection. When set, the review tool is resolved from the
    /// named tier's models list with heterogeneous preference. Takes priority
    /// over `tool` when both are set. The tier must exist in `[tiers]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// Default model for `csa review`. Overrides the tool's own default model
    /// selection (e.g., gemini-cli model steering) without requiring a tier.
    /// Without an active tier: CLI `--model` > this field > tool default.
    /// With an active review tier, the tier model spec remains authoritative unless CLI `--model` is provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Default thinking budget for `csa review` (`low`, `medium`, `high`, `xhigh`, `max`,
    /// or a token count).
    /// `csa review --thinking <LEVEL>` (when supported) overrides this.
    /// With an active review tier, the tier thinking budget remains authoritative unless CLI `--thinking` is provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Deprecated: prefer `gate_commands`. PROJECT-ONLY.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_command: Option<String>,
    /// Multi-layer gate pipeline (L1→L2→L3). Takes priority over `gate_command`. PROJECT-ONLY.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gate_commands: Vec<GateStep>,
    /// Timeout (seconds) for the pre-review quality gate command.
    ///
    /// PROJECT-ONLY: values set in global config are ignored during merge.
    #[serde(
        default = "default_gate_timeout_secs",
        skip_serializing_if = "is_default_gate_timeout"
    )]
    pub gate_timeout_secs: u64,
    /// When true, enforce filesystem-level read-only access to the project root
    /// during review sessions. This prevents the review tool from writing files
    /// even if instructed to. Default: false (allows resume-to-fix workflow).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly_sandbox: Option<bool>,
}

const fn default_gate_timeout_secs() -> u64 {
    250
}

const fn default_review_batch_commits() -> u32 {
    1
}

fn is_default_gate_timeout(val: &u64) -> bool {
    *val == default_gate_timeout_secs()
}

fn is_default_review_batch_commits(val: &u32) -> bool {
    *val == default_review_batch_commits()
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            tool: ToolSelection::default(),
            gate_mode: GateMode::default(),
            batch_commits: default_review_batch_commits(),
            tier: None,
            model: None,
            thinking: None,
            gate_command: None,
            gate_commands: Vec::new(),
            gate_timeout_secs: default_gate_timeout_secs(),
            readonly_sandbox: None,
        }
    }
}

impl ReviewConfig {
    /// Returns true when all fields match defaults (per rust/016 serde-default rule).
    pub fn is_default(&self) -> bool {
        self.tool.is_auto()
            && self.gate_mode == GateMode::Monitor
            && self.batch_commits == default_review_batch_commits()
            && self.tier.is_none()
            && self.model.is_none()
            && self.thinking.is_none()
            && self.gate_command.is_none()
            && self.gate_commands.is_empty()
            && self.gate_timeout_secs == default_gate_timeout_secs()
            && self.readonly_sandbox.is_none()
    }

    /// Returns the effective gate steps, preferring `gate_commands` over legacy
    /// `gate_command`. If both are empty, returns an empty vec.
    pub fn effective_gate_steps(&self) -> Vec<GateStep> {
        if !self.gate_commands.is_empty() {
            let mut steps = self.gate_commands.clone();
            steps.sort_by_key(|s| s.level);
            steps
        } else if let Some(cmd) = &self.gate_command {
            vec![GateStep {
                name: "legacy-gate".to_string(),
                command: cmd.clone(),
                level: 1,
            }]
        } else {
            Vec::new()
        }
    }

    /// Default gate timeout in seconds.
    pub const fn default_gate_timeout() -> u64 {
        default_gate_timeout_secs()
    }

    /// Default cumulative review batch size.
    pub const fn default_batch_commits() -> u32 {
        default_review_batch_commits()
    }
}

/// Configuration for the debate workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateConfig {
    /// Debate tool selection.
    ///
    /// Accepts a single tool name (`"codex"`), `"auto"` for heterogeneous
    /// auto-selection, or an array of tool names (`["codex", "gemini-cli"]`)
    /// as a whitelist for auto-selection. An empty array is equivalent to `"auto"`.
    #[serde(default)]
    pub tool: ToolSelection,
    /// Default absolute wall-clock timeout (seconds) for `csa debate`.
    ///
    /// `csa debate --timeout <N>` overrides this per invocation.
    #[serde(default = "default_debate_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Default model for `csa debate`. Overrides the tool's own default model
    /// selection (e.g., gemini-cli model steering) without requiring a tier.
    /// Without an active tier: CLI `--model` > this field > tool default.
    /// With an active debate tier, the tier model spec remains authoritative unless CLI `--model` is provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Default thinking budget for `csa debate` (`low`, `medium`, `high`, `xhigh`, `max`,
    /// or a token count).
    /// `csa debate --thinking <LEVEL>` overrides this per invocation.
    /// With an active debate tier, the tier thinking budget remains authoritative unless CLI `--thinking` is provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Allow same-model adversarial fallback when heterogeneous models are unavailable.
    ///
    /// When enabled (default), `csa debate` falls back to running two independent
    /// sub-agents of the same tool as Proposer and Critic. The debate output is
    /// annotated with "same-model adversarial" to indicate degraded diversity.
    ///
    /// Set to `false` to require heterogeneous models (strict mode).
    #[serde(default = "default_true_debate")]
    pub same_model_fallback: bool,
    /// Fail fast when a multi-tool debate tier collapses to a single surviving tool.
    ///
    /// Default: false. When true, tier resolution errors instead of silently
    /// proceeding with a narrowed single-tool panel.
    #[serde(default)]
    pub require_heterogeneous: bool,
    /// Tier-based tool selection. When set, the debate tool is resolved from the
    /// named tier's models list with heterogeneous preference. Takes priority
    /// over `tool` when both are set. The tier must exist in `[tiers]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// When true, enforce filesystem-level read-only access to the project root
    /// during debate sessions. This prevents the debate tool from writing files
    /// even if instructed to. Default: false (allows resume-to-fix workflow).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly_sandbox: Option<bool>,
}

fn default_debate_timeout_seconds() -> u64 {
    1800
}

fn default_true_debate() -> bool {
    true
}

impl Default for DebateConfig {
    fn default() -> Self {
        Self {
            tool: ToolSelection::default(),
            timeout_seconds: default_debate_timeout_seconds(),
            model: None,
            thinking: None,
            same_model_fallback: true,
            require_heterogeneous: false,
            tier: None,
            readonly_sandbox: None,
        }
    }
}

impl DebateConfig {
    /// Returns true when all fields match defaults (per rust/016 serde-default rule).
    pub fn is_default(&self) -> bool {
        self.tool.is_auto()
            && self.timeout_seconds == default_debate_timeout_seconds()
            && self.model.is_none()
            && self.thinking.is_none()
            && self.same_model_fallback
            && !self.require_heterogeneous
            && self.tier.is_none()
            && self.readonly_sandbox.is_none()
    }
}

/// Configuration for fallback behavior when external services are unavailable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackConfig {
    /// Behavior when cloud review bot is unavailable (quota, timeout, or API errors).
    ///
    /// - `"auto-local"`: Automatically fall back to local CSA review (still reviews)
    /// - `"ask-user"`: Prompt user before falling back (default)
    ///
    /// Both policies ensure code is still reviewed — `auto-local` just skips the
    /// user confirmation prompt. There is no `skip` option because bypassing
    /// review entirely violates the heterogeneous review safety model.
    #[serde(default = "default_cloud_review_exhausted")]
    pub cloud_review_exhausted: String,
}

fn default_cloud_review_exhausted() -> String {
    "ask-user".to_string()
}

impl Default for FallbackConfig {
    fn default() -> Self {
        Self {
            cloud_review_exhausted: default_cloud_review_exhausted(),
        }
    }
}

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

/// Returns all known tool names as a static slice.
pub fn all_known_tools() -> &'static [ToolName] {
    &[
        ToolName::GeminiCli,
        ToolName::Opencode,
        ToolName::Codex,
        ToolName::ClaudeCode,
        ToolName::OpenaiCompat,
    ]
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
    /// API key for fallback authentication. Used when OAuth quota is exhausted
    /// (e.g., gemini-cli falls back to API key auth after 429 retries fail).
    /// NOT injected into env by default — only used as a last resort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Allow gemini-cli to continue after disabling unhealthy MCP servers.
    /// Defaults to true so startup-time MCP degradation is non-fatal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_degraded_mcp: Option<bool>,
}

fn default_max_concurrent() -> u32 {
    DEFAULT_MAX_CONCURRENT
}

#[cfg(test)]
#[path = "global_tests.rs"]
mod tests;

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
