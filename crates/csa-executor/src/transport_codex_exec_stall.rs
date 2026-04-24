use crate::{executor::Executor, model_spec::ThinkingBudget};
use csa_process::ExecutionResult;

use super::transport_types::ResolvedTimeout;

pub const CODEX_EXEC_INITIAL_STALL_REASON: &str = "codex_exec_initial_stall";
pub const DEFAULT_CODEX_INITIAL_RESPONSE_TIMEOUT_SECONDS: u64 = 300;
const DEFAULT_GENERIC_INITIAL_RESPONSE_TIMEOUT_SECONDS: u64 = 120;

#[derive(Debug, Clone)]
pub struct CodexExecInitialStallClassification {
    pub(crate) effort: &'static str,
    pub(crate) timeout_seconds: u64,
    pub(crate) retry_effort: Option<ThinkingBudget>,
}

pub fn resolve_initial_response_timeout(
    configured_timeout_seconds: Option<u64>,
    default_if_none: u64,
) -> Option<u64> {
    match configured_timeout_seconds {
        None => Some(default_if_none),
        Some(0) => None,
        Some(seconds) => Some(seconds),
    }
}

fn executor_default_initial_response_timeout_seconds(executor: &Executor) -> u64 {
    if matches!(executor, Executor::Codex { .. }) {
        DEFAULT_CODEX_INITIAL_RESPONSE_TIMEOUT_SECONDS
    } else {
        DEFAULT_GENERIC_INITIAL_RESPONSE_TIMEOUT_SECONDS
    }
}

pub fn resolve_execute_in_initial_response_timeout_seconds(
    executor: &Executor,
    configured_timeout_seconds: Option<u64>,
) -> ResolvedTimeout {
    ResolvedTimeout(resolve_initial_response_timeout(
        configured_timeout_seconds,
        executor_default_initial_response_timeout_seconds(executor),
    ))
}

/// Consume an already-resolved watchdog setting without re-applying defaults.
///
/// The outer resolver is the single source of truth for defaulting/sentinel handling. Transport
/// internals must treat `None` as disabled and pass through positive values unchanged. `Some(0)`
/// is accepted defensively and treated as disabled so a stray sentinel cannot resurrect the codex
/// default.
pub(crate) fn consume_resolved_initial_response_timeout_seconds(
    resolved_timeout: ResolvedTimeout,
) -> Option<u64> {
    resolved_timeout.as_option().filter(|&seconds| seconds > 0)
}

/// Compatibility wrapper for direct `execute_in` callers/tests.
pub(crate) fn consume_resolved_execute_in_initial_response_timeout_seconds(
    resolved_timeout: ResolvedTimeout,
) -> Option<u64> {
    consume_resolved_initial_response_timeout_seconds(resolved_timeout)
}

pub fn classify_codex_exec_initial_stall(
    executor: &Executor,
    execution: &ExecutionResult,
    timeout_seconds: Option<u64>,
) -> Option<CodexExecInitialStallClassification> {
    if !matches!(executor, Executor::Codex { .. })
        || !execution.output.is_empty()
        || execution.exit_code != 137
        || !execution.summary.starts_with("initial_response_timeout:")
    {
        return None;
    }

    let Executor::Codex {
        thinking_budget, ..
    } = executor
    else {
        unreachable!("guarded above");
    };

    let budget = thinking_budget
        .clone()
        .unwrap_or(ThinkingBudget::DefaultBudget);
    Some(CodexExecInitialStallClassification {
        effort: budget.codex_effort(),
        timeout_seconds: timeout_seconds.unwrap_or(DEFAULT_CODEX_INITIAL_RESPONSE_TIMEOUT_SECONDS),
        retry_effort: budget.codex_stall_retry_downgrade(),
    })
}

pub fn apply_codex_exec_initial_stall_summary(
    execution: &mut ExecutionResult,
    classification: &CodexExecInitialStallClassification,
    retry_attempted: bool,
    original_effort: Option<&str>,
) {
    let mut summary = format!(
        "{CODEX_EXEC_INITIAL_STALL_REASON}: no stdout within {}s (effort={}, retry_attempted={retry_attempted}",
        classification.timeout_seconds, classification.effort
    );
    if let Some(original_effort) = original_effort {
        summary.push_str(&format!(", original_effort={original_effort}"));
    }
    summary.push(')');

    execution.summary = summary.clone();
    if !execution.stderr_output.is_empty() && !execution.stderr_output.ends_with('\n') {
        execution.stderr_output.push('\n');
    }
    execution.stderr_output.push_str(&summary);
    execution.stderr_output.push('\n');
}
