use std::path::Path;

use csa_acp::SessionEvent;
use csa_process::{ExecutionResult, StreamMode};
use csa_resource::isolation_plan::IsolationPlan;
use serde::{Deserialize, Serialize};

use crate::model_spec::ThinkingBudget;

/// Signals that the initial-response timeout has already passed through a resolver.
///
/// `None` means the watchdog is disabled; `Some(seconds)` is the concrete deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResolvedTimeout(pub Option<u64>);

impl ResolvedTimeout {
    pub const fn disabled() -> Self {
        Self(None)
    }

    pub const fn of(secs: u64) -> Self {
        Self(Some(secs))
    }

    pub const fn as_option(&self) -> Option<u64> {
        self.0
    }
}

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
    pub initial_response_timeout: ResolvedTimeout,
    pub liveness_dead_seconds: u64,
    pub stdin_write_timeout_seconds: u64,
    pub acp_init_timeout_seconds: u64,
    pub termination_grace_period_seconds: u64,
    pub output_spool: Option<&'a Path>,
    pub output_spool_max_bytes: u64,
    pub output_spool_keep_rotated: bool,
    pub setting_sources: Option<Vec<String>>,
    pub sandbox: Option<&'a SandboxTransportConfig>,
    /// Current thinking budget for idle-disconnect auto-downshift (Issue #766).
    /// When set and an ACP idle disconnect is detected, the retry uses a
    /// one-level-lower budget. `None` disables idle-disconnect downshift.
    pub thinking_budget: Option<ThinkingBudget>,
}

#[derive(Debug, Clone)]
pub struct TransportResult {
    pub execution: ExecutionResult,
    pub provider_session_id: Option<String>,
    pub events: Vec<SessionEvent>,
    pub metadata: csa_acp::StreamingMetadata,
}

/// Informational capability matrix for a Transport implementation.
/// Used by `csa doctor` and Phase 2 config validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TransportCapabilities {
    /// Streams agent events (ACP: true, legacy CLI: false).
    pub streaming: bool,
    /// Can resume a prior session by ID.
    pub session_resume: bool,
    /// Can fork an existing session (claude-code via ACP only).
    pub session_fork: bool,
    /// Emits typed event variants (vs raw stdout/stderr).
    pub typed_events: bool,
}

pub(super) fn should_stream_acp_stdout_to_stderr(
    stream_mode: StreamMode,
    output_spool: Option<&Path>,
) -> bool {
    !(matches!(stream_mode, StreamMode::BufferOnly)
        || output_spool.is_some() && std::env::var_os("CSA_DAEMON_SESSION_ID").is_some())
}
