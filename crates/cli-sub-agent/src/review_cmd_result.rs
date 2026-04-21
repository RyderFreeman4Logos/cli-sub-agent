use anyhow::Result;
use csa_core::types::{ReviewDecision, ToolName};
use tracing::warn;

use crate::review_consensus::{
    CLEAN, HAS_ISSUES, SKIP, UNAVAILABLE, UNCERTAIN, parse_review_decision,
};

use super::execute::ReviewExecutionOutcome;
use super::output::{
    GEMINI_AUTH_PROMPT_STATUS_REASON, ReviewerOutcome, detect_tool_diagnostic,
    is_review_output_empty, sanitize_review_output,
};

const AUTH_PROMPT_REVIEW_UNAVAILABLE: &str = "Review unavailable: gemini-cli OAuth prompt detected; authentication required (no review verdict produced).\n";
const AUTH_PROMPT_DIAGNOSTIC: &str =
    "gemini-cli auth failure: OAuth browser prompt detected; no review verdict produced";
const REVIEW_UNAVAILABLE_PREFIX: &str = "Review unavailable: ";

fn verdict_from_decision(decision: ReviewDecision) -> &'static str {
    match decision {
        ReviewDecision::Pass => CLEAN,
        ReviewDecision::Fail => HAS_ISSUES,
        ReviewDecision::Skip => SKIP,
        ReviewDecision::Uncertain => UNCERTAIN,
        ReviewDecision::Unavailable => UNAVAILABLE,
    }
}

fn exit_code_from_decision(decision: ReviewDecision) -> i32 {
    match decision {
        ReviewDecision::Pass => 0,
        ReviewDecision::Fail
        | ReviewDecision::Skip
        | ReviewDecision::Uncertain
        | ReviewDecision::Unavailable => 1,
    }
}

pub(super) struct SingleReviewResolution {
    pub sanitized: String,
    pub empty_output: bool,
    pub verdict: &'static str,
    pub decision: ReviewDecision,
    pub effective_exit_code: i32,
    pub auth_prompt_failure: bool,
}

pub(super) fn resolve_single_review_result(
    result: &ReviewExecutionOutcome,
    tool: ToolName,
    scope: &str,
) -> SingleReviewResolution {
    let auth_prompt_failure =
        result.status_reason.as_deref() == Some(GEMINI_AUTH_PROMPT_STATUS_REASON);
    let forced_unavailable = matches!(result.forced_decision, Some(ReviewDecision::Unavailable));
    let sanitized = if auth_prompt_failure {
        AUTH_PROMPT_REVIEW_UNAVAILABLE.to_string()
    } else if forced_unavailable {
        format!(
            "{REVIEW_UNAVAILABLE_PREFIX}{}\n",
            result
                .failure_reason
                .as_deref()
                .unwrap_or("all configured tier models failed")
        )
    } else {
        sanitize_review_output(&result.execution.execution.output)
    };
    let empty_output = !auth_prompt_failure
        && !forced_unavailable
        && is_review_output_empty(&result.execution.execution.output);
    let tool_diagnostic = detect_tool_diagnostic(
        &result.execution.execution.output,
        &result.execution.execution.stderr_output,
    );
    if empty_output {
        if let Some(ref diagnostic) = tool_diagnostic {
            eprintln!("[csa-review] Tool failure detected: {diagnostic}");
        }
        warn!(scope = %scope, tool = %tool, session_id = %result.execution.meta_session_id,
            diagnostic = tool_diagnostic.as_deref().unwrap_or("unknown"),
            "Review produced no substantive output — tool may have failed silently. \
             Check: csa session logs {}", result.execution.meta_session_id);
    } else if let Some(ref diagnostic) = tool_diagnostic {
        eprintln!("[csa-review] Warning: {diagnostic}");
        warn!(scope = %scope, tool = %tool,
            "Tool diagnostic detected in review output (review may be degraded)");
    }

    let decision = if let Some(forced) = result.forced_decision {
        forced
    } else if auth_prompt_failure || empty_output {
        ReviewDecision::Uncertain
    } else {
        parse_review_decision(
            &result.execution.execution.output,
            result.execution.execution.exit_code,
        )
    };
    let verdict = verdict_from_decision(decision);
    let effective_exit_code = exit_code_from_decision(decision);

    SingleReviewResolution {
        sanitized,
        empty_output,
        verdict,
        decision,
        effective_exit_code,
        auth_prompt_failure,
    }
}

pub(super) fn build_reviewer_outcome(
    reviewer_index: usize,
    reviewer_tool: ToolName,
    session_result: &ReviewExecutionOutcome,
) -> Result<ReviewerOutcome> {
    let result = &session_result.execution;
    let auth_prompt_failure =
        session_result.status_reason.as_deref() == Some(GEMINI_AUTH_PROMPT_STATUS_REASON);
    let forced_unavailable = matches!(
        session_result.forced_decision,
        Some(ReviewDecision::Unavailable)
    );
    let empty = !auth_prompt_failure
        && !forced_unavailable
        && is_review_output_empty(&result.execution.output);
    let diagnostic =
        detect_tool_diagnostic(&result.execution.output, &result.execution.stderr_output);
    if empty {
        warn!(
            reviewer = reviewer_index + 1,
            tool = %reviewer_tool,
            diagnostic = diagnostic.as_deref().unwrap_or("unknown"),
            "Reviewer produced no substantive output — tool may have failed"
        );
    }

    Ok(ReviewerOutcome {
        reviewer_index,
        tool: reviewer_tool,
        session_id: session_result.execution.meta_session_id.clone(),
        verdict: verdict_from_decision(if let Some(forced) = session_result.forced_decision {
            forced
        } else if auth_prompt_failure || empty {
            ReviewDecision::Uncertain
        } else {
            parse_review_decision(&result.execution.output, result.execution.exit_code)
        }),
        output: if auth_prompt_failure {
            AUTH_PROMPT_REVIEW_UNAVAILABLE.to_string()
        } else if forced_unavailable {
            format!(
                "{REVIEW_UNAVAILABLE_PREFIX}{}\n",
                session_result
                    .failure_reason
                    .as_deref()
                    .unwrap_or("all configured tier models failed")
            )
        } else {
            sanitize_review_output(&result.execution.output)
        },
        exit_code: exit_code_from_decision(if let Some(forced) = session_result.forced_decision {
            forced
        } else if auth_prompt_failure || empty {
            ReviewDecision::Uncertain
        } else {
            parse_review_decision(&result.execution.output, result.execution.exit_code)
        }),
        diagnostic: diagnostic
            .or_else(|| auth_prompt_failure.then(|| AUTH_PROMPT_DIAGNOSTIC.to_string())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::SessionExecutionResult;
    use csa_process::ExecutionResult;

    fn outcome(output: &str, exit_code: i32) -> ReviewExecutionOutcome {
        ReviewExecutionOutcome {
            execution: SessionExecutionResult {
                execution: ExecutionResult {
                    output: output.to_string(),
                    stderr_output: String::new(),
                    summary: String::new(),
                    exit_code,
                    peak_memory_mb: None,
                },
                meta_session_id: "01TESTRESULT".to_string(),
                provider_session_id: None,
            },
            executed_tool: ToolName::Codex,
            status_reason: None,
            forced_decision: None,
            routed_to: None,
            primary_failure: None,
            failure_reason: None,
        }
    }

    #[test]
    fn resolve_single_review_result_preserves_uncertain_contract() {
        let resolved = resolve_single_review_result(
            &outcome(
                "<!-- CSA:SECTION:summary -->\nUNCERTAIN\n<!-- CSA:SECTION:summary:END -->",
                0,
            ),
            ToolName::Codex,
            "uncommitted",
        );

        assert_eq!(resolved.decision, ReviewDecision::Uncertain);
        assert_eq!(resolved.verdict, UNCERTAIN);
        assert_eq!(resolved.effective_exit_code, 1);
    }

    #[test]
    fn build_reviewer_outcome_preserves_skip_contract() {
        let reviewer = build_reviewer_outcome(
            0,
            ToolName::Codex,
            &outcome(
                "<!-- CSA:SECTION:summary -->\nSKIP\n<!-- CSA:SECTION:summary:END -->",
                0,
            ),
        )
        .expect("reviewer outcome");

        assert_eq!(reviewer.verdict, SKIP);
        assert_eq!(reviewer.exit_code, 1);
    }
}
