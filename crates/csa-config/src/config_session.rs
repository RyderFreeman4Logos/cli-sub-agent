use csa_core::vcs::VcsKind;
use serde::{Deserialize, Serialize};

use crate::config_tool::default_true;

/// Session management configuration (`[session]` in config).
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
    /// Maximum stderr spool file size in megabytes before rotation (default 50).
    ///
    /// stderr is typically more verbose than stdout (tracing output, tee'd lines)
    /// so the default is larger.  Set to `None` to inherit `spool_max_mb`.
    #[serde(default)]
    pub stderr_spool_max_mb: Option<u32>,
    /// Keep rotated spool files for debugging (default true).
    #[serde(default)]
    pub spool_keep_rotated: Option<bool>,
    /// Enable tool output compression for large outputs (default false, opt-in).
    ///
    /// When enabled, tool outputs exceeding `tool_output_threshold_bytes` are
    /// replaced in-context with a file path reference. The full output is
    /// persisted to `{session_dir}/tool_outputs/` for on-demand retrieval.
    #[serde(default)]
    pub tool_output_compression: bool,
    /// Byte threshold above which tool outputs are compressed (default 8192).
    ///
    /// Only effective when `tool_output_compression` is enabled.
    #[serde(default = "default_tool_output_threshold_bytes")]
    pub tool_output_threshold_bytes: u64,
    /// Timeout (seconds) for `csa session wait` polling loop.
    ///
    /// The default of 250s is intentional: it lets the daemon's KV cache stay
    /// warm while periodically returning control to the calling orchestrator.
    /// The caller is expected to re-invoke `csa session wait` in a loop.
    #[serde(default = "default_daemon_wait_seconds")]
    pub daemon_wait_seconds: u64,
    /// Cooldown period (seconds) between consecutive session launches.
    ///
    /// Prevents rapid-fire session creation that can exhaust API quotas or
    /// trigger provider rate limits. Set to `0` to disable cooldown entirely.
    #[serde(default = "default_cooldown_secs")]
    pub cooldown_seconds: u64,
    /// Timeout (seconds) for joining the stderr drain thread during daemon shutdown.
    ///
    /// If a child process inherits the daemon's stderr pipe and outlives the daemon,
    /// the drain thread blocks on `read(pipe)` indefinitely.  This timeout prevents
    /// daemon shutdown from hanging.  Default: 5 seconds.
    #[serde(default = "default_stderr_drain_timeout_secs")]
    pub stderr_drain_timeout_secs: u64,
}

fn default_seed_max_age_secs() -> u64 {
    86400 // 24 hours
}

fn default_max_seed_sessions() -> u32 {
    2
}

fn default_tool_output_threshold_bytes() -> u64 {
    8192
}

/// Default daemon wait timeout: 250s for KV cache warmth.
pub const DEFAULT_DAEMON_WAIT_SECS: u64 = 250;

/// Default cooldown between consecutive session launches (seconds).
///
/// Prevents rapid-fire session creation that can exhaust API quotas or
/// trigger provider rate limits. Set to `0` to disable cooldown entirely.
pub const DEFAULT_COOLDOWN_SECS: u64 = 10;

fn default_daemon_wait_seconds() -> u64 {
    DEFAULT_DAEMON_WAIT_SECS
}

fn default_cooldown_secs() -> u64 {
    DEFAULT_COOLDOWN_SECS
}

/// Default drain thread join timeout: 5 seconds.
pub const DEFAULT_STDERR_DRAIN_TIMEOUT_SECS: u64 = 5;

fn default_stderr_drain_timeout_secs() -> u64 {
    DEFAULT_STDERR_DRAIN_TIMEOUT_SECS
}

const DEFAULT_SPOOL_MAX_MB: u32 = 32;
const DEFAULT_STDERR_SPOOL_MAX_MB: u32 = 50;
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
            stderr_spool_max_mb: None,
            spool_keep_rotated: None,
            tool_output_compression: false,
            tool_output_threshold_bytes: default_tool_output_threshold_bytes(),
            daemon_wait_seconds: default_daemon_wait_seconds(),
            cooldown_seconds: default_cooldown_secs(),
            stderr_drain_timeout_secs: default_stderr_drain_timeout_secs(),
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
            && self.stderr_spool_max_mb.is_none()
            && self.spool_keep_rotated.is_none()
            && !self.tool_output_compression
            && self.tool_output_threshold_bytes == default_tool_output_threshold_bytes()
            && self.daemon_wait_seconds == default_daemon_wait_seconds()
            && self.cooldown_seconds == default_cooldown_secs()
            && self.stderr_drain_timeout_secs == default_stderr_drain_timeout_secs()
    }

    /// Resolve cooldown duration (0 = disabled).
    pub fn cooldown_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.cooldown_seconds)
    }

    pub fn resolved_spool_max_mb(&self) -> u32 {
        self.spool_max_mb.unwrap_or(DEFAULT_SPOOL_MAX_MB)
    }

    pub fn resolved_stderr_spool_max_mb(&self) -> u32 {
        // Fallback chain: stderr_spool_max_mb → spool_max_mb → 50 MiB default.
        // This matches the field comment ("None means inherit spool_max_mb").
        self.stderr_spool_max_mb
            .or(self.spool_max_mb)
            .unwrap_or(DEFAULT_STDERR_SPOOL_MAX_MB)
    }

    pub fn resolved_stderr_drain_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.stderr_drain_timeout_secs)
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
    #[serde(
        default = "default_acp_crash_max_attempts",
        skip_serializing_if = "is_default_acp_crash_max_attempts"
    )]
    pub acp_crash_max_attempts: u8,
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

const fn default_acp_crash_max_attempts() -> u8 {
    2
}

fn is_default_acp_crash_max_attempts(val: &u8) -> bool {
    *val == default_acp_crash_max_attempts()
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            min_timeout_seconds: default_min_timeout_seconds(),
            acp_crash_max_attempts: default_acp_crash_max_attempts(),
            auto_weave_upgrade: false,
        }
    }
}

impl ExecutionConfig {
    /// Returns true when all fields are at their defaults (per rust/016 serde-default rule).
    pub fn is_default(&self) -> bool {
        self.min_timeout_seconds == default_min_timeout_seconds()
            && self.acp_crash_max_attempts == default_acp_crash_max_attempts()
            && !self.auto_weave_upgrade
    }

    /// The compile-time default minimum timeout in seconds.
    pub const fn default_min_timeout() -> u64 {
        default_min_timeout_seconds()
    }

    pub fn resolved_acp_crash_max_attempts(&self) -> u8 {
        self.acp_crash_max_attempts.clamp(1, 5)
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
