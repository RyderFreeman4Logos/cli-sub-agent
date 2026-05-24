use super::*;
use crate::pipeline::SessionExecutionResult;
use csa_process::ExecutionResult;
use std::{fs, path::Path};

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
            changed_paths: None,
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

    let resolved =
        resolve_single_review_result(&result, ToolName::GeminiCli, "uncommitted", Path::new("."));

    assert_eq!(resolved.decision, ReviewDecision::Unavailable);
    assert_eq!(resolved.verdict, UNAVAILABLE);
    assert_eq!(resolved.effective_exit_code, 1);
    assert!(resolved.sanitized.contains("Review unavailable:"));
    assert!(resolved.sanitized.contains("retryDelayMs: undefined"));
}

#[test]
fn build_reviewer_outcome_maps_quota_limited_tool_to_unavailable() {
    let mut result = outcome(
        r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"seven_day"}}"#,
        1,
    );
    result.execution.execution.summary =
        r#"{"api_error_status":429,"result":"You've hit your org's monthly usage limit"}"#
            .to_string();

    let reviewer = build_reviewer_outcome(0, ToolName::Codex, &result).expect("reviewer outcome");

    assert_eq!(reviewer.verdict, UNAVAILABLE);
    assert_eq!(reviewer.exit_code, 1);
    assert!(reviewer.output.contains("Review unavailable:"));
    assert!(
        reviewer
            .diagnostic
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.contains("monthly usage limit"))
    );
}

#[test]
fn build_reviewer_outcome_maps_api_key_failure_to_unavailable() {
    let mut result = outcome("", 1);
    result.executed_tool = ToolName::GeminiCli;
    result.execution.execution.stderr_output = r#"API Key not found.
details: [{"reason":"API_KEY_INVALID","domain":"googleapis.com"}]"#
        .to_string();

    let reviewer =
        build_reviewer_outcome(0, ToolName::GeminiCli, &result).expect("reviewer outcome");

    assert_eq!(reviewer.verdict, UNAVAILABLE);
    assert_eq!(reviewer.exit_code, 1);
    assert!(reviewer.output.contains("Review unavailable:"));
    assert!(
        reviewer
            .diagnostic
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.contains("API_KEY_INVALID"))
    );
}

#[test]
fn reviewer_unavailable_error_reason_maps_quota_exhausted_error() {
    let err = anyhow::anyhow!(
        "gemini-cli failed: status: RESOURCE_EXHAUSTED; reason: QUOTA_EXHAUSTED; HTTP 429"
    );

    let reason =
        reviewer_unavailable_error_reason(&err, ToolName::GeminiCli).expect("quota reason");

    assert!(reason.contains("gemini-cli tool failure"));
    assert!(reason.contains("QUOTA_EXHAUSTED"));
}

#[test]
fn reviewer_unavailable_error_reason_maps_api_key_invalid_error() {
    let err = anyhow::anyhow!("gemini-cli failed: status 400 API_KEY_INVALID");

    let reason = reviewer_unavailable_error_reason(&err, ToolName::GeminiCli).expect("auth reason");

    assert!(reason.contains("gemini-cli tool failure"));
    assert!(reason.contains("API_KEY_INVALID"));
}

#[test]
fn resolve_single_review_result_maps_api_key_failure_to_unavailable() {
    let mut result = outcome("", 1);
    result.executed_tool = ToolName::GeminiCli;
    result.execution.execution.stderr_output =
        "status: 400 API_KEY_INVALID API Key not found".to_string();

    let resolved =
        resolve_single_review_result(&result, ToolName::GeminiCli, "uncommitted", Path::new("."));

    assert_eq!(resolved.decision, ReviewDecision::Unavailable);
    assert_eq!(resolved.verdict, UNAVAILABLE);
    assert_eq!(resolved.effective_exit_code, 1);
    assert!(resolved.sanitized.contains("Review unavailable:"));
}

#[test]
fn build_reviewer_outcome_does_not_mark_review_prose_quota_mentions_unavailable() {
    let reviewer = build_reviewer_outcome(
        0,
        ToolName::Codex,
        &outcome(
            "Substantive review finding: quota handling drops errors but no explicit verdict.",
            1,
        ),
    )
    .expect("reviewer outcome");

    assert_ne!(reviewer.verdict, UNAVAILABLE);
    assert!(!reviewer.output.contains("Review unavailable:"));
    assert!(reviewer.diagnostic.is_none());
}

#[test]
fn tool_unavailable_failure_does_not_override_explicit_fail_verdict() {
    let reviewer = build_reviewer_outcome(
        0,
        ToolName::Codex,
        &outcome(
            "<!-- CSA:SECTION:summary -->\nFAIL\n<!-- CSA:SECTION:summary:END -->\n\
             <!-- CSA:SECTION:details -->\nA real finding mentions rate limit handling.\n<!-- CSA:SECTION:details:END -->",
            1,
        ),
    )
    .expect("reviewer outcome");

    assert_eq!(reviewer.verdict, HAS_ISSUES);
    assert!(!reviewer.output.contains("Review unavailable:"));
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
    fs::write(session_dir.join("output").join("summary.md"), "**PASS**\n").expect("write summary");

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

#[test]
fn forced_unavailable_preserves_explicit_pass_verdict() {
    let mut result = outcome(
        "<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n\
         <!-- CSA:SECTION:details -->\nfindings = []\n<!-- CSA:SECTION:details:END -->",
        1,
    );
    result.forced_decision = Some(ReviewDecision::Unavailable);
    result.failure_reason =
        Some("claude-code post-review state write failed: EROFS ~/.claude.json".to_string());

    let resolved =
        resolve_single_review_result(&result, ToolName::ClaudeCode, "uncommitted", Path::new("."));
    assert_eq!(resolved.decision, ReviewDecision::Pass);
    assert_eq!(resolved.verdict, CLEAN);
    assert_eq!(resolved.effective_exit_code, 0);

    let reviewer =
        build_reviewer_outcome(0, ToolName::ClaudeCode, &result).expect("reviewer outcome");
    assert_eq!(reviewer.verdict, CLEAN);
    assert_eq!(reviewer.exit_code, 0);
}
