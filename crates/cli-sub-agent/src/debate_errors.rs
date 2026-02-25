use std::path::Path;

use csa_process::{ExecutionResult, ToolLiveness};
use csa_session::MetaSessionState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DebateErrorKind {
    Transient(String),
    Deterministic(String),
    StillWorking,
}

pub(crate) fn classify_execution_outcome(
    execution: &ExecutionResult,
    session_state: Option<&MetaSessionState>,
    session_dir: &Path,
) -> DebateErrorKind {
    if ToolLiveness::is_alive(session_dir) {
        return DebateErrorKind::StillWorking;
    }

    let termination_reason = session_state.and_then(|s| s.termination_reason.as_deref());
    let sandbox_memory_limit = session_state
        .and_then(|s| s.sandbox_info.as_ref())
        .and_then(|s| s.memory_max_mb);

    if execution.exit_code == 137 {
        if matches!(termination_reason, Some("sigkill" | "sigterm"))
            || sandbox_memory_limit.is_some()
        {
            return DebateErrorKind::Transient(format!(
                "exit 137 (termination_reason={:?}, sandbox_memory_max_mb={:?})",
                termination_reason, sandbox_memory_limit
            ));
        }
        return DebateErrorKind::Deterministic(format!(
            "exit 137 without transient signal (termination_reason={:?})",
            termination_reason
        ));
    }

    if execution.exit_code == 143 || matches!(termination_reason, Some("sigterm" | "sigint")) {
        return DebateErrorKind::Transient(format!(
            "external signal (exit_code={}, termination_reason={:?})",
            execution.exit_code, termination_reason
        ));
    }

    // Catch-all: valid signal-based exit codes (128+signal) are transient.
    // Valid Unix signals: 1-31 (standard) + 32-64 (real-time), so exit
    // codes 129-192.  128 (signal 0) and 193+ (signal > 64) are not real
    // signal exits — treat those as deterministic.
    // CSA only sends SIGTERM(15) and SIGKILL(9); other signals come from
    // external sources (systemd scope cleanup, kernel OOM, etc.).
    if execution.exit_code >= 129 && execution.exit_code <= 192 {
        let signal_num = execution.exit_code - 128;
        tracing::warn!(
            exit_code = execution.exit_code,
            signal = signal_num,
            "process killed by unexpected signal; classifying as transient"
        );
        return DebateErrorKind::Transient(format!(
            "process killed by signal {} (exit_code={})",
            signal_num, execution.exit_code,
        ));
    }

    let stderr_lower = execution.stderr_output.to_ascii_lowercase();
    if stderr_lower.contains("permission denied") {
        return DebateErrorKind::Deterministic("permission error".to_string());
    }

    if execution.exit_code == 1 {
        return DebateErrorKind::Deterministic("argument error (exit code 1)".to_string());
    }

    DebateErrorKind::Deterministic(format!("exit code {}", execution.exit_code))
}

pub(crate) fn classify_execution_error(
    error: &anyhow::Error,
    session_dir: Option<&Path>,
) -> DebateErrorKind {
    if session_dir.is_some_and(ToolLiveness::is_alive) {
        return DebateErrorKind::StillWorking;
    }

    let message = error.to_string().to_ascii_lowercase();
    if message.contains("oom")
        || message.contains("signal")
        || message.contains("killed")
        || message.contains("temporarily unavailable")
    {
        return DebateErrorKind::Transient(error.to_string());
    }

    if message.contains("permission denied") {
        return DebateErrorKind::Deterministic("permission error".to_string());
    }

    DebateErrorKind::Deterministic(error.to_string())
}

#[cfg(test)]
#[path = "debate_errors_tests.rs"]
mod tests;
