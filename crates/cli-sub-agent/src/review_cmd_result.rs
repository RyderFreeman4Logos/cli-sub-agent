use anyhow::Result;
use csa_core::{
    gemini::RATE_LIMIT_PATTERNS,
    types::{ReviewDecision, ToolName},
};
use std::fs;
use std::path::Path;
use tracing::warn;

use crate::review_consensus::{
    CLEAN, HAS_ISSUES, SKIP, UNAVAILABLE, UNCERTAIN, parse_explicit_review_decision_token,
    parse_review_decision,
};

use super::execute::ReviewExecutionOutcome;
use super::output::{
    GEMINI_AUTH_PROMPT_STATUS_REASON, ReviewerOutcome, detect_tool_diagnostic, extract_review_text,
    is_review_output_empty, sanitize_review_output, stream_started_without_terminal_event,
};

const AUTH_PROMPT_REVIEW_UNAVAILABLE: &str = "Review unavailable: gemini-cli OAuth prompt detected; authentication required (no review verdict produced).\n";
const AUTH_PROMPT_DIAGNOSTIC: &str =
    "gemini-cli auth failure: OAuth browser prompt detected; no review verdict produced";
const REVIEW_UNAVAILABLE_PREFIX: &str = "Review unavailable: ";
const REVIEW_UNAVAILABLE_FAILURE_PATTERNS: &[&str] = &[
    "retrydelayms",
    "rate limit",
    "rate_limit",
    "usage limit",
    "monthly usage limit",
    "overloaded",
    "temporarily unavailable",
    "503",
    "status: 400",
    "status 400",
    "http 400",
    "bad request",
    "invalid request",
    "invalid_request_error",
    "api key not found",
    "api_key_invalid",
    "invalid api key",
    "authentication required",
];

fn verdict_from_decision(decision: ReviewDecision) -> &'static str {
    match decision {
        ReviewDecision::Pass => CLEAN,
        ReviewDecision::Fail => HAS_ISSUES,
        ReviewDecision::Skip => SKIP,
        ReviewDecision::Uncertain => UNCERTAIN,
        ReviewDecision::Unavailable => UNAVAILABLE,
    }
}

/// Whether a reviewer subprocess was killed/timed-out mid-turn: its streamed transcript began
/// but never reached a terminal completion event, and it exited non-zero. Such a reviewer never
/// produced a deliberate verdict — any verdict token in its partial output is unreliable (often
/// scraped from the reviewed code) — so it must be classified `Unavailable` (infra could not run
/// to completion) rather than fail-closed to `HAS_ISSUES` (#1657).
fn reviewer_killed_before_completion(raw_output: &str, exit_code: i32) -> bool {
    exit_code != 0 && stream_started_without_terminal_event(raw_output)
}

fn explicit_review_decision_for_execution(
    raw_output: &str,
    exit_code: i32,
    summary_fallback: Option<&str>,
) -> Option<ReviewDecision> {
    let review_text = extract_review_text(raw_output).unwrap_or_else(|| raw_output.to_string());
    if parse_explicit_review_decision_token(&review_text).is_some() {
        return Some(parse_review_decision(&review_text, exit_code));
    }

    summary_fallback
        .filter(|summary| parse_explicit_review_decision_token(summary).is_some())
        .map(|summary| parse_review_decision(summary, 0))
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
    project_root: &Path,
) -> SingleReviewResolution {
    let auth_prompt_failure =
        result.status_reason.as_deref() == Some(GEMINI_AUTH_PROMPT_STATUS_REASON);
    let forced_unavailable = matches!(result.forced_decision, Some(ReviewDecision::Unavailable));
    let tool_unavailable_reason = tool_unavailable_failure_reason(result, tool);
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
    } else if let Some(reason) = tool_unavailable_reason.as_deref() {
        format!("{REVIEW_UNAVAILABLE_PREFIX}{reason}\n")
    } else {
        sanitize_review_output(&result.execution.execution.output)
    };
    let empty_output = !auth_prompt_failure
        && !forced_unavailable
        && tool_unavailable_reason.is_none()
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

    let explicit_decision = explicit_review_decision_for_execution(
        &result.execution.execution.output,
        result.execution.execution.exit_code,
        load_summary_fallback(project_root, &result.execution.meta_session_id).as_deref(),
    );

    let decision = if reviewer_killed_before_completion(
        &result.execution.execution.output,
        result.execution.execution.exit_code,
    ) {
        // Reviewer killed/timed-out mid-turn: treat as infra-unavailable, not a scraped verdict.
        ReviewDecision::Unavailable
    } else if let Some(decision) = explicit_decision {
        decision
    } else if let Some(forced) = result.forced_decision {
        forced
    } else if tool_unavailable_reason.is_some() {
        ReviewDecision::Unavailable
    } else if auth_prompt_failure || empty_output {
        ReviewDecision::Uncertain
    } else {
        parse_review_decision_for_execution(
            &result.execution.execution.output,
            result.execution.execution.exit_code,
            load_summary_fallback(project_root, &result.execution.meta_session_id).as_deref(),
        )
    };
    let verdict = verdict_from_decision(decision);
    let effective_exit_code = crate::verdict_exit_code::exit_code_from_review_decision(decision);

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
    let tool_unavailable_reason = tool_unavailable_failure_reason(session_result, reviewer_tool);
    let empty = !auth_prompt_failure
        && !forced_unavailable
        && tool_unavailable_reason.is_none()
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

    let explicit_decision = explicit_review_decision_for_execution(
        &result.execution.output,
        result.execution.exit_code,
        None,
    );
    let decision = if reviewer_killed_before_completion(
        &result.execution.output,
        result.execution.exit_code,
    ) {
        // Reviewer killed/timed-out mid-turn (no terminal stream event + non-zero exit): the
        // reviewer never produced a deliberate verdict, so any verdict token in its partial
        // output is unreliable. Classify as infra-unavailable so the all-reviewers-unavailable
        // case persists `unavailable` and a producing co-reviewer can still gate the merge (#1657).
        ReviewDecision::Unavailable
    } else if let Some(decision) = explicit_decision {
        decision
    } else if let Some(forced) = session_result.forced_decision {
        forced
    } else if tool_unavailable_reason.is_some() {
        ReviewDecision::Unavailable
    } else if auth_prompt_failure || empty {
        ReviewDecision::Uncertain
    } else {
        parse_review_decision_for_execution(
            &result.execution.output,
            result.execution.exit_code,
            None,
        )
    };

    Ok(ReviewerOutcome {
        reviewer_index,
        tool: reviewer_tool,
        session_id: session_result.execution.meta_session_id.clone(),
        verdict: verdict_from_decision(decision),
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
        } else if let Some(reason) = tool_unavailable_reason.as_deref() {
            format!("{REVIEW_UNAVAILABLE_PREFIX}{reason}\n")
        } else {
            sanitize_review_output(&result.execution.output)
        },
        exit_code: crate::verdict_exit_code::exit_code_from_review_decision(decision),
        diagnostic: diagnostic
            .or_else(|| auth_prompt_failure.then(|| AUTH_PROMPT_DIAGNOSTIC.to_string()))
            .or_else(|| tool_unavailable_reason.clone()),
    })
}

pub(super) fn reviewer_unavailable_error_reason(
    err: &anyhow::Error,
    tool: ToolName,
) -> Option<String> {
    let error_text = format!("{err:#}");
    contains_review_unavailable_failure_pattern(&error_text).then(|| {
        format!(
            "{} tool failure: {}",
            tool.as_str(),
            truncate_single_line(&error_text, 240)
        )
    })
}

fn tool_unavailable_failure_reason(
    result: &ReviewExecutionOutcome,
    tool: ToolName,
) -> Option<String> {
    if result.execution.execution.exit_code == 0 {
        return None;
    }
    if extract_review_text(&result.execution.execution.output)
        .as_deref()
        .is_some_and(|text| parse_explicit_review_decision_token(text).is_some())
    {
        return None;
    }

    let fields = [
        result.failure_reason.as_deref(),
        result.primary_failure.as_deref(),
        Some(result.execution.execution.summary.as_str()),
        Some(result.execution.execution.stderr_output.as_str()),
        result.status_reason.as_deref(),
    ];

    fields
        .into_iter()
        .flatten()
        .find(|text| contains_review_unavailable_failure_pattern(text))
        .map(|text| {
            format!(
                "{} tool failure: {}",
                tool.as_str(),
                truncate_single_line(text, 240)
            )
        })
}

fn contains_review_unavailable_failure_pattern(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    REVIEW_UNAVAILABLE_FAILURE_PATTERNS
        .iter()
        .chain(RATE_LIMIT_PATTERNS.iter())
        .any(|pattern| lower.contains(pattern))
        || lower.contains("quota")
}

fn truncate_single_line(text: &str, max_chars: usize) -> String {
    text.chars()
        .take(max_chars)
        .collect::<String>()
        .trim()
        .replace('\n', " ")
}

fn parse_review_decision_for_execution(
    raw_output: &str,
    exit_code: i32,
    summary_fallback: Option<&str>,
) -> ReviewDecision {
    if let Some(decision) =
        explicit_review_decision_for_execution(raw_output, exit_code, summary_fallback)
    {
        return decision;
    }
    if let Some(summary) = summary_fallback {
        return parse_review_decision(summary, 0);
    }

    let review_text = extract_review_text(raw_output).unwrap_or_else(|| raw_output.to_string());
    parse_review_decision(&review_text, exit_code)
}

fn load_summary_fallback(project_root: &Path, session_id: &str) -> Option<String> {
    let session_dir = csa_session::get_session_dir(project_root, session_id).ok()?;
    fs::read_to_string(session_dir.join("output").join("summary.md")).ok()
}

pub(super) fn build_unavailable_reviewer_outcome(
    reviewer_index: usize,
    reviewer_tool: ToolName,
    reason: impl Into<String>,
) -> ReviewerOutcome {
    let reason = reason.into();
    ReviewerOutcome {
        reviewer_index,
        tool: reviewer_tool,
        session_id: format!("reviewer-{}-unavailable", reviewer_index + 1),
        output: format!("{REVIEW_UNAVAILABLE_PREFIX}{reason}\n"),
        exit_code: crate::verdict_exit_code::exit_code_from_review_decision(
            ReviewDecision::Unavailable,
        ),
        verdict: UNAVAILABLE,
        diagnostic: Some(reason),
    }
}

#[cfg(test)]
#[path = "review_cmd_result_tests.rs"]
mod tests;
