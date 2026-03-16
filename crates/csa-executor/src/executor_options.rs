use std::path::PathBuf;

use csa_process::StreamMode;

/// Options for tool execution, including stream mode, timeouts, and optional sandbox config.
#[derive(Debug, Clone)]
pub struct ExecuteOptions {
    pub stream_mode: StreamMode,
    pub idle_timeout_seconds: u64,
    pub liveness_dead_seconds: u64,
    pub stdin_write_timeout_seconds: u64,
    pub acp_init_timeout_seconds: u64,
    pub termination_grace_period_seconds: u64,
    pub output_spool: Option<PathBuf>,
    pub output_spool_max_bytes: u64,
    pub output_spool_keep_rotated: bool,
    /// Selective MCP/setting sources for ACP session meta.
    /// `Some(sources)` → inject `settingSources` into session meta.
    /// `None` → no override (load everything).
    pub setting_sources: Option<Vec<String>>,
    /// Optional resource sandbox config (cgroup/rlimit limits).
    /// When `Some`, the spawned tool process will be wrapped in resource isolation.
    pub sandbox: Option<SandboxContext>,
}

/// Sandbox configuration resolved from project/tool config.
///
/// Carries the resource limits together with identifiers needed to name
/// the cgroup scope (tool name + session ID).
#[derive(Debug, Clone)]
pub struct SandboxContext {
    /// Resource limits to apply.
    pub config: csa_resource::cgroup::SandboxConfig,
    /// Tool name for scope naming (e.g. "claude-code").
    pub tool_name: String,
    /// Session ID for scope naming.
    pub session_id: String,
    /// When true, sandbox spawn failures fall back to unsandboxed spawn.
    pub best_effort: bool,
}

impl ExecuteOptions {
    pub fn new(stream_mode: StreamMode, idle_timeout_seconds: u64) -> Self {
        Self {
            stream_mode,
            idle_timeout_seconds,
            liveness_dead_seconds: csa_process::DEFAULT_LIVENESS_DEAD_SECS,
            stdin_write_timeout_seconds: csa_process::DEFAULT_STDIN_WRITE_TIMEOUT_SECS,
            acp_init_timeout_seconds: 120,
            termination_grace_period_seconds: csa_process::DEFAULT_TERMINATION_GRACE_PERIOD_SECS,
            output_spool: None,
            output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
            output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
            setting_sources: None,
            sandbox: None,
        }
    }

    /// Override stdin write timeout (seconds) for spawned child processes.
    pub fn with_stdin_write_timeout_seconds(mut self, seconds: u64) -> Self {
        self.stdin_write_timeout_seconds = seconds;
        self
    }

    /// Override liveness dead timeout (seconds) for idle-timeout liveness mode.
    pub fn with_liveness_dead_seconds(mut self, seconds: u64) -> Self {
        self.liveness_dead_seconds = seconds;
        self
    }

    /// Override ACP initialization timeout (seconds).
    pub fn with_acp_init_timeout_seconds(mut self, seconds: u64) -> Self {
        self.acp_init_timeout_seconds = seconds;
        self
    }

    /// Override termination grace period (seconds) for forced shutdown.
    pub fn with_termination_grace_period_seconds(mut self, seconds: u64) -> Self {
        self.termination_grace_period_seconds = seconds;
        self
    }

    /// Set selective MCP/setting sources for ACP session meta.
    pub fn with_setting_sources(mut self, setting_sources: Option<Vec<String>>) -> Self {
        self.setting_sources = setting_sources;
        self
    }

    /// Set sandbox context for resource isolation.
    pub fn with_sandbox(mut self, sandbox: SandboxContext) -> Self {
        self.sandbox = Some(sandbox);
        self
    }

    /// Set output spool file path for incremental/final output persistence.
    pub fn with_output_spool(mut self, output_spool: PathBuf) -> Self {
        self.output_spool = Some(output_spool);
        self
    }

    /// Override spool rotation behavior for output.log artifacts.
    pub fn with_output_spool_rotation(mut self, max_bytes: u64, keep_rotated: bool) -> Self {
        self.output_spool_max_bytes = max_bytes;
        self.output_spool_keep_rotated = keep_rotated;
        self
    }
}
