use anyhow::Result;
use csa_core::{
    gemini::RATE_LIMIT_PATTERNS,
    types::{ReviewDecision, ToolName},
};
use std::fs;
use std::path::Path;
use tracing::warn;

use crate::review_consensus::{
    CLEAN, HAS_ISSUES, SKIP, UNAVAILABLE, UNCERTAIN, parse_review_decision,
};

use super::execute::ReviewExecutionOutcome;
use super::output::{
    GEMINI_AUTH_PROMPT_STATUS_REASON, ReviewerOutcome, detect_tool_diagnostic, extract_review_text,
    is_review_output_empty, sanitize_review_output,
};

const AUTH_PROMPT_REVIEW_UNAVAILABLE: &str = "Review unavailable: gemini-cli OAuth prompt detected; authentication required (no review verdict produced).\n";
const AUTH_PROMPT_DIAGNOSTIC: &str =
    "gemini-cli auth failure: OAuth browser prompt detected; no review verdict produced";
const REVIEW_UNAVAILABLE_PREFIX: &str = "Review unavailable: ";
const TRANSIENT_GEMINI_FAILURE_PATTERNS: &[&str] = &[
    "retrydelayms",
    "rate limit",
    "rate_limit",
    "overloaded",
    "temporarily unavailable",
    "503",
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
    project_root: &Path,
) -> SingleReviewResolution {
    let auth_prompt_failure =
        result.status_reason.as_deref() == Some(GEMINI_AUTH_PROMPT_STATUS_REASON);
    let forced_unavailable = matches!(result.forced_decision, Some(ReviewDecision::Unavailable));
    let transient_unavailable_reason = transient_gemini_failure_reason(result, tool);
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
    } else if let Some(reason) = transient_unavailable_reason.as_deref() {
        format!("{REVIEW_UNAVAILABLE_PREFIX}{reason}\n")
    } else {
        sanitize_review_output(&result.execution.execution.output)
    };
    let empty_output = !auth_prompt_failure
        && !forced_unavailable
        && transient_unavailable_reason.is_none()
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
    } else if transient_unavailable_reason.is_some() {
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
    let transient_unavailable_reason =
        transient_gemini_failure_reason(session_result, reviewer_tool);
    let empty = !auth_prompt_failure
        && !forced_unavailable
        && transient_unavailable_reason.is_none()
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
        } else if transient_unavailable_reason.is_some() {
            ReviewDecision::Unavailable
        } else if auth_prompt_failure || empty {
            ReviewDecision::Uncertain
        } else {
            parse_review_decision_for_execution(
                &result.execution.output,
                result.execution.exit_code,
                None,
            )
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
        } else if let Some(reason) = transient_unavailable_reason.as_deref() {
            format!("{REVIEW_UNAVAILABLE_PREFIX}{reason}\n")
        } else {
            sanitize_review_output(&result.execution.output)
        },
        exit_code: exit_code_from_decision(if let Some(forced) = session_result.forced_decision {
            forced
        } else if transient_unavailable_reason.is_some() {
            ReviewDecision::Unavailable
        } else if auth_prompt_failure || empty {
            ReviewDecision::Uncertain
        } else {
            parse_review_decision_for_execution(
                &result.execution.output,
                result.execution.exit_code,
                None,
            )
        }),
        diagnostic: diagnostic
            .or_else(|| auth_prompt_failure.then(|| AUTH_PROMPT_DIAGNOSTIC.to_string()))
            .or_else(|| transient_unavailable_reason.clone()),
    })
}

fn transient_gemini_failure_reason(
    result: &ReviewExecutionOutcome,
    tool: ToolName,
) -> Option<String> {
    if tool != ToolName::GeminiCli || result.execution.execution.exit_code == 0 {
        return None;
    }

    let fields = [
        result.failure_reason.as_deref(),
        result.primary_failure.as_deref(),
        Some(result.execution.execution.summary.as_str()),
        Some(result.execution.execution.stderr_output.as_str()),
        Some(result.execution.execution.output.as_str()),
        result.status_reason.as_deref(),
    ];

    fields
        .into_iter()
        .flatten()
        .find(|text| contains_transient_gemini_failure_pattern(text))
        .map(|text| {
            format!(
                "gemini-cli transient failure: {}",
                truncate_single_line(text, 240)
            )
        })
}

fn contains_transient_gemini_failure_pattern(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    TRANSIENT_GEMINI_FAILURE_PATTERNS
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
    let review_text = extract_review_text(raw_output).unwrap_or_else(|| raw_output.to_string());
    if !contains_explicit_verdict_token(&review_text)
        && let Some(summary) = summary_fallback
    {
        return parse_review_decision(summary, 0);
    }
    parse_review_decision(&review_text, exit_code)
}

fn load_summary_fallback(project_root: &Path, session_id: &str) -> Option<String> {
    let session_dir = csa_session::get_session_dir(project_root, session_id).ok()?;
    fs::read_to_string(session_dir.join("output").join("summary.md")).ok()
}

fn contains_explicit_verdict_token(text: &str) -> bool {
    [
        "HAS_ISSUES",
        "FAIL",
        "UNAVAILABLE",
        "UNCERTAIN",
        "CLEAN",
        "PASS",
        "SKIP",
    ]
    .iter()
    .any(|token| contains_token(text, token))
}

fn contains_token(haystack: &str, token: &str) -> bool {
    haystack
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|part| part.eq_ignore_ascii_case(token))
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
        exit_code: 1,
        verdict: UNAVAILABLE,
        diagnostic: Some(reason),
    }
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
            persistable_session_id: Some("01TESTRESULT".to_string()),
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
            Path::new("."),
        );

        assert_eq!(resolved.decision, ReviewDecision::Uncertain);
        assert_eq!(resolved.verdict, UNCERTAIN);
        assert_eq!(resolved.effective_exit_code, 1);
    }

    #[test]
    fn resolve_single_review_result_maps_transient_gemini_failure_to_unavailable() {
        let mut result = outcome("", 1);
        result.executed_tool = ToolName::GeminiCli;
        result.execution.execution.summary = "retryDelayMs: undefined".to_string();

        let resolved = resolve_single_review_result(
            &result,
            ToolName::GeminiCli,
            "uncommitted",
            Path::new("."),
        );

        assert_eq!(resolved.decision, ReviewDecision::Unavailable);
        assert_eq!(resolved.verdict, UNAVAILABLE);
        assert_eq!(resolved.effective_exit_code, 1);
        assert!(resolved.sanitized.contains("Review unavailable:"));
        assert!(resolved.sanitized.contains("retryDelayMs: undefined"));
    }

    #[test]
    fn resolve_single_review_result_uses_last_reviewer_message_not_prompt_tokens() {
        let raw = concat!(
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Reviewer instructions mention HAS_ISSUES as a legacy alias.\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"<!-- CSA:SECTION:summary -->\\nPASS\\n<!-- CSA:SECTION:summary:END -->\\n\\n<!-- CSA:SECTION:details -->\\nNo blocking correctness issues found.\\n<!-- CSA:SECTION:details:END -->\"}}\n",
        );

        let resolved = resolve_single_review_result(
            &outcome(raw, 0),
            ToolName::Codex,
            "uncommitted",
            Path::new("."),
        );

        assert_eq!(resolved.decision, ReviewDecision::Pass);
        assert_eq!(resolved.verdict, CLEAN);
        assert_eq!(resolved.effective_exit_code, 0);
    }

    #[test]
    fn resolve_single_review_result_falls_back_to_summary_md_for_clear_verdict() {
        let project_root = tempfile::tempdir().expect("temp project");
        let session_dir = csa_session::get_session_root(project_root.path())
            .expect("session root")
            .join("sessions")
            .join("01TESTRESULT");
        fs::create_dir_all(session_dir.join("output")).expect("create output dir");
        fs::write(session_dir.join("output").join("summary.md"), "**PASS**\n")
            .expect("write summary");

        let resolved = resolve_single_review_result(
            &outcome("review completed without a verdict token", 1),
            ToolName::Codex,
            "uncommitted",
            project_root.path(),
        );

        assert_eq!(resolved.decision, ReviewDecision::Pass);
        assert_eq!(resolved.verdict, CLEAN);
    }

    #[test]
    fn mock_pass_review_persists_pass_review_meta() {
        let project_root = tempfile::tempdir().expect("temp project");
        let session_dir = csa_session::get_session_root(project_root.path())
            .expect("session root")
            .join("sessions")
            .join("01TESTRESULT");
        fs::create_dir_all(session_dir.join("output")).expect("create output dir");
        let raw = concat!(
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Prompt context: legacy HAS_ISSUES maps to failure.\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"<!-- CSA:SECTION:summary -->\\nPASS\\n<!-- CSA:SECTION:summary:END -->\\n\\n<!-- CSA:SECTION:details -->\\nNo blocking correctness issues found.\\n<!-- CSA:SECTION:details:END -->\"}}\n",
        );
        let resolved = resolve_single_review_result(
            &outcome(raw, 0),
            ToolName::Codex,
            "uncommitted",
            project_root.path(),
        );
        let meta = csa_session::state::ReviewSessionMeta {
            session_id: "01TESTRESULT".to_string(),
            head_sha: "HEAD".to_string(),
            decision: resolved.decision.as_str().to_string(),
            verdict: resolved.verdict.to_string(),
            status_reason: None,
            routed_to: None,
            primary_failure: None,
            failure_reason: None,
            tool: ToolName::Codex.to_string(),
            scope: "uncommitted".to_string(),
            exit_code: resolved.effective_exit_code,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 0,
            timestamp: chrono::Utc::now(),
            diff_fingerprint: None,
        };

        csa_session::state::write_review_meta(&session_dir, &meta).expect("write review meta");
        let persisted: csa_session::state::ReviewSessionMeta = serde_json::from_str(
            &fs::read_to_string(session_dir.join("review_meta.json")).expect("read review meta"),
        )
        .expect("parse review meta");

        assert_eq!(persisted.decision, ReviewDecision::Pass.as_str());
        assert_eq!(persisted.verdict, CLEAN);
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
