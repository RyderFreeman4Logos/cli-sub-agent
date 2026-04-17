use crate::{executor::Executor, model_spec::ThinkingBudget};
use csa_process::ExecutionResult;

pub(crate) const CODEX_EXEC_INITIAL_STALL_REASON: &str = "codex_exec_initial_stall";
const DEFAULT_CODEX_INITIAL_RESPONSE_TIMEOUT_SECONDS: u64 = 300;

#[derive(Debug, Clone)]
pub(crate) struct CodexExecInitialStallClassification {
    pub(crate) effort: &'static str,
    pub(crate) timeout_seconds: u64,
    pub(crate) retry_effort: Option<ThinkingBudget>,
}

pub(crate) fn codex_initial_response_timeout_seconds(
    executor: &Executor,
    configured_timeout_seconds: Option<u64>,
) -> Option<u64> {
    if matches!(executor, Executor::Codex { .. }) {
        configured_timeout_seconds.filter(|&seconds| seconds > 0)
    } else {
        configured_timeout_seconds.filter(|&seconds| seconds > 0)
    }
}

pub(crate) fn resolve_execute_in_initial_response_timeout_seconds(
    executor: &Executor,
    configured_timeout_seconds: Option<u64>,
) -> Option<u64> {
    let configured_timeout_seconds = configured_timeout_seconds.filter(|&seconds| seconds > 0);
    if matches!(executor, Executor::Codex { .. }) {
        configured_timeout_seconds.or(Some(DEFAULT_CODEX_INITIAL_RESPONSE_TIMEOUT_SECONDS))
    } else {
        configured_timeout_seconds
    }
}

pub(crate) fn classify_codex_exec_initial_stall(
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

    let budget = match executor {
        Executor::Codex {
            thinking_budget, ..
        } => thinking_budget
            .clone()
            .unwrap_or(ThinkingBudget::DefaultBudget),
        _ => ThinkingBudget::DefaultBudget,
    };
    Some(CodexExecInitialStallClassification {
        effort: budget.codex_effort(),
        timeout_seconds: timeout_seconds.unwrap_or(DEFAULT_CODEX_INITIAL_RESPONSE_TIMEOUT_SECONDS),
        retry_effort: matches!(budget, ThinkingBudget::Xhigh).then_some(ThinkingBudget::High),
    })
}

pub(crate) fn apply_codex_exec_initial_stall_summary(
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
