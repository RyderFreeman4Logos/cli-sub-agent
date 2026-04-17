use anyhow::Result;
use csa_core::types::{ReviewDecision, ToolName};
use tracing::warn;

use crate::review_consensus::{HAS_ISSUES, parse_review_decision, parse_review_verdict};

use super::execute::ReviewExecutionOutcome;
use super::output::{
    GEMINI_AUTH_PROMPT_STATUS_REASON, ReviewerOutcome, detect_tool_diagnostic,
    is_review_output_empty, sanitize_review_output,
};

const AUTH_PROMPT_REVIEW_UNAVAILABLE: &str = "Review unavailable: gemini-cli OAuth prompt detected; authentication required (no review verdict produced).\n";
const AUTH_PROMPT_DIAGNOSTIC: &str =
    "gemini-cli auth failure: OAuth browser prompt detected; no review verdict produced";

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
    let sanitized = if auth_prompt_failure {
        AUTH_PROMPT_REVIEW_UNAVAILABLE.to_string()
    } else {
        sanitize_review_output(&result.execution.execution.output)
    };
    let empty_output =
        !auth_prompt_failure && is_review_output_empty(&result.execution.execution.output);
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

    let verdict = if auth_prompt_failure {
        "UNCERTAIN"
    } else if empty_output {
        HAS_ISSUES
    } else {
        parse_review_verdict(
            &result.execution.execution.output,
            result.execution.execution.exit_code,
        )
    };
    let decision = if auth_prompt_failure || empty_output {
        ReviewDecision::Uncertain
    } else {
        parse_review_decision(
            &result.execution.execution.output,
            result.execution.execution.exit_code,
        )
    };
    let effective_exit_code = if empty_output {
        1
    } else {
        result.execution.execution.exit_code
    };

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
    let empty = !auth_prompt_failure && is_review_output_empty(&result.execution.output);
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
        verdict: if auth_prompt_failure {
            "UNCERTAIN"
        } else if empty {
            HAS_ISSUES
        } else {
            parse_review_verdict(&result.execution.output, result.execution.exit_code)
        },
        output: if auth_prompt_failure {
            AUTH_PROMPT_REVIEW_UNAVAILABLE.to_string()
        } else {
            sanitize_review_output(&result.execution.output)
        },
        exit_code: if empty || auth_prompt_failure {
            1
        } else {
            result.execution.exit_code
        },
        diagnostic: diagnostic
            .or_else(|| auth_prompt_failure.then(|| AUTH_PROMPT_DIAGNOSTIC.to_string())),
    })
}
