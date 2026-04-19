use std::future::Future;

use crate::executor::Executor;
use crate::model_spec::ThinkingBudget;
use anyhow::Result;

use super::transport_codex_exec_stall::CodexExecInitialStallClassification;
use super::{
    CODEX_EXEC_INITIAL_STALL_REASON, TransportResult, apply_codex_exec_initial_stall_summary,
    classify_codex_exec_initial_stall,
};

pub(super) fn log_codex_exec_initial_stall(
    classification: &CodexExecInitialStallClassification,
    child_pid: Option<u32>,
) {
    tracing::warn!(
        classified_reason = CODEX_EXEC_INITIAL_STALL_REASON,
        elapsed_seconds = classification.timeout_seconds,
        effort = classification.effort,
        child_pid = child_pid.unwrap_or(0),
        "codex exec initial-response stall detected"
    );
}

pub(super) async fn apply_and_maybe_retry_codex_exec_initial_stall<F, Fut>(
    executor: &Executor,
    result: TransportResult,
    timeout_seconds: Option<u64>,
    retry_once: F,
) -> Result<TransportResult>
where
    F: FnOnce(ThinkingBudget) -> Fut,
    Fut: Future<Output = Result<(Executor, TransportResult)>>,
{
    let Some(classification) =
        classify_codex_exec_initial_stall(executor, &result.execution, timeout_seconds)
    else {
        return Ok(result);
    };

    if let Some(retry_budget) = classification.retry_effort.clone() {
        tracing::info!(
            original_effort = classification.effort,
            fallback_effort = retry_budget.codex_effort(),
            "retrying codex exec after initial-response stall"
        );
        let (downgraded_executor, mut retry_result) = retry_once(retry_budget).await?;
        if let Some(retry_classification) = classify_codex_exec_initial_stall(
            &downgraded_executor,
            &retry_result.execution,
            timeout_seconds,
        ) {
            apply_codex_exec_initial_stall_summary(
                &mut retry_result.execution,
                &retry_classification,
                true,
                Some(classification.effort),
            );
        }
        return Ok(retry_result);
    }

    let mut result = result;
    apply_codex_exec_initial_stall_summary(&mut result.execution, &classification, false, None);
    Ok(result)
}
