use std::path::Path;

use csa_acp::SessionEvent;
use csa_process::{ExecutionResult, StreamMode};
use csa_resource::isolation_plan::IsolationPlan;

#[derive(Debug, Clone)]
pub struct SandboxTransportConfig {
    pub isolation_plan: IsolationPlan,
    pub tool_name: String,
    pub best_effort: bool,
    pub session_id: String,
}

#[derive(Debug, Clone)]
pub struct TransportOptions<'a> {
    pub stream_mode: StreamMode,
    pub idle_timeout_seconds: u64,
    pub acp_crash_max_attempts: u8,
    /// Already resolved at the outer pipeline / executor boundary.
    ///
    /// Contract:
    /// - `None` disables the watchdog
    /// - `Some(seconds > 0)` arms the watchdog for that duration
    /// - `Some(0)` is tolerated defensively and treated as disabled by transport consumers
    pub initial_response_timeout_seconds: Option<u64>,
    pub liveness_dead_seconds: u64,
    pub stdin_write_timeout_seconds: u64,
    pub acp_init_timeout_seconds: u64,
    pub termination_grace_period_seconds: u64,
    pub output_spool: Option<&'a Path>,
    pub output_spool_max_bytes: u64,
    pub output_spool_keep_rotated: bool,
    pub setting_sources: Option<Vec<String>>,
    pub sandbox: Option<&'a SandboxTransportConfig>,
}

#[derive(Debug, Clone)]
pub struct TransportResult {
    pub execution: ExecutionResult,
    pub provider_session_id: Option<String>,
    pub events: Vec<SessionEvent>,
    pub metadata: csa_acp::StreamingMetadata,
}

pub(super) fn should_stream_acp_stdout_to_stderr(
    stream_mode: StreamMode,
    output_spool: Option<&Path>,
) -> bool {
    !(matches!(stream_mode, StreamMode::BufferOnly)
        || output_spool.is_some() && std::env::var_os("CSA_DAEMON_SESSION_ID").is_some())
}
